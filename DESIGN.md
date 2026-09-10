# Design — Dodo Invoice Service

## 1. Overview

Dodo Invoice Service is a Rust-based invoice and payment service designed around correctness, concurrency safety, idempotent payment processing, and asynchronous webhook delivery.

The system uses:

* **Rust** for the application
* **Axum** for HTTP APIs
* **PostgreSQL** for persistent storage
* **SQLx** for database access and migrations
* **Tokio** for asynchronous execution and background processing
* A **Mock PSP** to simulate payment success and failure scenarios

The design prioritizes correctness of financial state transitions over premature optimization.

---

# 2. Data Model

## 2.1 Businesses

Stores the merchant/business account.

```text
businesses
-----------
id
name
created_at
updated_at
```

A business is the top-level tenant in the system.

All business-owned resources are scoped through `business_id`.

---

## 2.2 API Keys

Stores credentials used to authenticate API requests.

```text
api_keys
--------
id
business_id
key_prefix
key_hash
created_at
revoked_at
```

The API key prefix is stored in plaintext so that requests can efficiently locate the corresponding key record.

The secret portion is stored as an **Argon2id hash** rather than plaintext.

Revocation is represented by `revoked_at`.

Authentication therefore follows:

```text
Authorization: Bearer <API_KEY>
             │
             ▼
       Extract prefix
             │
             ▼
      Find API key record
             │
             ▼
      Check revoked_at
             │
             ▼
      Verify Argon2id hash
             │
             ▼
       Resolve business
```

---

## 2.3 Customers

```text
customers
---------
id
business_id
name
email
created_at
updated_at
```

Customers belong to a specific business.

Customer lookup and creation are therefore always scoped to the authenticated business.

The application checks for duplicate `(business_id, email)` combinations.

A database-level unique constraint should also be used to eliminate races between concurrent create requests.

---

## 2.4 Invoices

```text
invoices
--------
id
business_id
customer_id
state
total_cents
due_date
created_at
updated_at
paid_at
version
```

Important design decisions:

### Money

Money is represented using integer cents.

```text
$10.50 → 1050
```

Floating-point values are deliberately avoided in the payment path.

### Server-side total calculation

`total_cents` is calculated by the service from invoice line items rather than being trusted from client input.

### Version

The invoice contains a `version` field that can be used for optimistic concurrency when updating invoice state.

---

## 2.5 Line Items

Line items belong to an invoice.

Amounts and quantities use integer representations:

```text
quantity
unit_amount_cents
```

This keeps monetary calculations deterministic.

---

## 2.6 Payment Attempts

```text
payment_attempts
----------------
id
invoice_id
idempotency_key
psp_reference
status
error_code
error_message
requested_at
completed_at
created_at
```

Each payment attempt is stored rather than overwriting previous attempts.

This provides an audit trail for:

* successful payments
* declined payments
* insufficient funds
* PSP failures
* timeouts
* retries

The payment attempt therefore represents the interaction with the PSP while the invoice remains the source of truth for the final business state.

---

## 2.7 Webhook Endpoints

```text
webhook_endpoints
-----------------
id
business_id
url
secret
created_at
updated_at
```

Each endpoint belongs to a business.

Webhook authentication uses a secret associated with the endpoint.

The endpoint registration design treats `(business_id, url)` as the natural uniqueness boundary.

---

## 2.8 Webhook Deliveries

Webhook deliveries are persisted separately from webhook endpoint configuration.

A delivery records the state of an individual event delivery and allows the worker to retry failed deliveries using backoff.

This prevents webhook delivery failures from affecting the main payment transaction.

---

# 3. Invoice State Machine

Invoices use the following states:

```text
draft
open
paid
void
uncollectible
```

`paid`, `void`, and `uncollectible` are terminal states.

```text
                 ┌──────────────┐
                 │    draft     │
                 └──────┬───────┘
                        │
                    MarkOpen
                        │
                        ▼
                 ┌──────────────┐
                 │     open     │
                 └──┬────┬───┬──┘
                    │    │   │
             MarkPaid│    │   │MarkUncollectible
                    │    │   │
                    ▼    │   ▼
                 ┌─────┐ │ ┌────────────────┐
                 │paid │ │ │ uncollectible  │
                 └─────┘ │ └────────────────┘
                  terminal│       terminal
                          │
                       MarkVoid
                          │
                          ▼
                       ┌──────┐
                       │ void │
                       └──────┘
                       terminal
```

Both `draft` and `open` are considered payable by the payment logic.

State transitions are centralized in:

```text
state_machine::validate_transition
```

This avoids duplicating transition rules across individual handlers.

Invalid transitions return:

```text
409 Conflict
```

with an `invalid_state_transition` error.

Terminal states cannot be transitioned back into another state.

---

# 4. Payment Flow

The payment flow is intentionally transaction-oriented.

```text
POST /api/invoices/{id}/pay
            │
            ▼
     Authenticate API key
            │
            ▼
       Begin transaction
            │
            ▼
     Lock invoice row
     SELECT ... FOR UPDATE
            │
            ▼
      Check invoice state
            │
       ┌────┴────┐
       │         │
    payable   already paid
       │         │
       │         └──────► 409
       ▼
 Check idempotency key
       │
       ▼
 Create payment attempt
       │
       ▼
    Call PSP
       │
   ┌───┴──────────────┐
   │                  │
Success              Failure
   │                  │
   ▼                  ▼
Mark paid        Mark failed
   │                  │
   ▼                  ▼
Commit          Keep invoice open
   │                  │
   └────────┬─────────┘
            ▼
       Trigger webhook
```

The invoice row is locked during the critical payment transaction.

This provides deterministic behavior when multiple requests attempt to pay the same invoice concurrently.

---

# 5. Payment Correctness

## 5.1 Concurrent Payment Requests

Consider two requests arriving simultaneously:

```text
Request A ───────► Pay Invoice
Request B ───────► Pay Invoice
```

Both attempt to acquire the invoice lock:

```sql
SELECT ...
FROM invoices
WHERE id = $1
FOR UPDATE;
```

Only one transaction can hold the row lock at a time.

The first request proceeds with payment.

After it commits:

```text
invoice.state = paid
```

The second request then obtains the lock and sees the updated state.

It fails the payable-state check and returns:

```text
409 already_paid
```

This prevents two successful payment operations against the same invoice.

### Trade-off

The PSP request occurs while the invoice transaction holds the lock.

This simplifies correctness but means the database connection and row lock remain occupied while waiting for the PSP.

The PSP timeout therefore becomes important for bounding the duration of the lock.

---

# 6. PSP Timeout Handling

The Mock PSP provides a timeout scenario:

```text
tok_timeout
```

The mock waits approximately 30 seconds before returning.

The PSP client uses a 5-second HTTP timeout.

Therefore:

```text
Payment request
      │
      ▼
Mock PSP
      │
      │ waits 30 seconds
      │
      X
   5-second
    timeout
      │
      ▼
PspError
      │
      ▼
Payment attempt → failed
Invoice          → remains open
Webhook          → payment_failed
```

The customer does not have to wait for the full 30 seconds.

They receive a failure after the configured PSP timeout.

### Reconciliation limitation

The Mock PSP does not expose a charge-status lookup API.

Therefore, after a timeout, the service cannot determine whether the PSP eventually completed the charge.

In production, this should be solved with a PSP reconciliation mechanism that queries the PSP's authoritative payment status.

---

# 7. PSP Success Followed by Service Failure

A distributed payment flow cannot guarantee atomicity between:

```text
Application database
        +
External PSP
```

For example:

```text
1. Application sends payment to PSP
2. PSP returns success
3. Application starts persisting success
4. Application crashes before database commit
```

The PSP may have successfully charged the customer while the local database still contains:

```text
payment_attempt.status = pending
```

Because the Mock PSP does not provide a status lookup API, the current implementation cannot reconcile this scenario automatically.

A production implementation should periodically identify stale pending attempts and reconcile them with the PSP.

---

# 8. Idempotency

Payment requests support an idempotency key.

Conceptually:

```text
(invoice_id, idempotency_key)
```

identifies a logical payment request.

For a retry using the same key:

```text
First request
     │
     ▼
Create payment attempt
     │
     ▼
Process PSP
     │
     ▼
Store result


Retry with same key
     │
     ▼
Find existing attempt
     │
     ▼
Return existing result
```

This prevents a normal client retry from creating another PSP payment.

The implementation performs the existing-attempt lookup before starting a new payment attempt.

### Concurrency consideration

The lookup alone is not sufficient to guarantee idempotency for two truly simultaneous requests.

A database-level unique constraint on:

```text
(invoice_id, idempotency_key)
```

is the appropriate final protection against a lookup/insert race.

This should be enforced by the database rather than relying exclusively on application-level checks.

---

# 9. Paying an Already-Paid Invoice

Once an invoice reaches:

```text
paid
```

it becomes terminal.

A subsequent payment request:

```text
POST /api/invoices/{id}/pay
```

is rejected with:

```text
409 already_paid
```

The request does not create another payment attempt or call the PSP.

The row lock ensures that this check observes the latest committed invoice state.

---

# 10. Webhook Architecture

Webhook delivery is intentionally separated from the payment request.

The desired flow is:

```text
Payment transaction
       │
       ▼
Commit database changes
       │
       ▼
Create webhook delivery
       │
       ▼
Background worker
       │
       ▼
HTTP webhook request
       │
   ┌───┴────┐
   │        │
 Success   Failure
   │        │
   ▼        ▼
Delivered  Retry
            │
            ▼
          Backoff
```

A slow webhook receiver must not block the payment request.

The invoice and payment records remain the authoritative source of truth.

Webhooks are notifications of state changes, not the source of financial truth.

---

# 11. Webhook Security

Webhook payloads are signed using:

```text
HMAC-SHA256
```

Each webhook endpoint has its own secret.

The secret is generated during endpoint registration and is intended to be returned to the business at registration time rather than exposed through normal endpoint retrieval.

Consumers can use the secret to verify that webhook requests originated from the service.

Conceptually:

```text
payload + endpoint secret
          │
          ▼
      HMAC-SHA256
          │
          ▼
   signature header
          │
          ▼
   Receiver verifies
```

---

# 12. Webhook Reliability

Webhook delivery is asynchronous.

Failed deliveries are retried using backoff, with a bounded number of attempts.

This provides resilience against temporary failures such as:

* receiver downtime
* network failures
* HTTP 5xx responses
* temporary connectivity problems

The webhook system should remain best-effort from the perspective of notification delivery.

The invoice database remains the source of truth.

---

# 13. Authentication and Tenant Isolation

Every authenticated request resolves a business from its API key.

The resulting business identity is then used to scope database operations.

Conceptually:

```text
API Key
   │
   ▼
Authenticate
   │
   ▼
business_id
   │
   ▼
Database queries
   │
   ▼
WHERE business_id = authenticated_business
```

This prevents one business from accessing another business's:

* customers
* invoices
* payment attempts
* webhook endpoints

API keys therefore have a business-level blast radius.

If a key is compromised, access is limited to the business associated with that key.

---

# 14. API Key Security

API keys follow a format similar to:

```text
dk_<prefix>_<secret>
```

The prefix is stored for efficient lookup.

The secret portion is hashed using:

```text
Argon2id
```

The complete plaintext key is not stored.

Requests use:

```text
Authorization: Bearer <key>
```

Revocation is represented by `revoked_at`.

Authentication checks that the key has not been revoked.

There is no separate rotation operation required by the design.

Rotation can be performed by:

```text
Create new key
      ↓
Update client
      ↓
Revoke old key
```

---

# 15. Database Transactions

Transactions are used for operations where multiple database changes must be treated as one logical unit.

Payment processing is the most important example.

The transaction coordinates:

```text
Invoice state
      +
Payment attempt
      +
Payment result
```

The goal is to avoid states such as:

```text
invoice = paid
payment_attempt = failed
```

or:

```text
invoice = open
payment_attempt = succeeded
```

being committed as the normal result of a single successful payment operation.

The external PSP remains outside PostgreSQL's transaction boundary, which is why reconciliation is still required for certain failure windows.

---

# 16. Failure Scenarios Supported by the Mock PSP

The Mock PSP supports deterministic scenarios for testing:

| Token                    | Behavior                       |
| ------------------------ | ------------------------------ |
| `tok_success`            | Successful payment             |
| `tok_insufficient_funds` | Insufficient funds             |
| `tok_card_declined`      | Card declined                  |
| `tok_timeout`            | 30-second delayed response     |
| `tok_network_error`      | HTTP 500/network-style failure |
| Invalid token            | Invalid token response         |

This makes the payment service testable without depending on an external payment provider.

---

# 17. What Is Intentionally Out of Scope

The current implementation deliberately does not attempt to implement every feature of a production payment platform.

### Refunds

Refunds and partial refunds are outside the assessment scope.

### Multi-currency

The implementation focuses on a single currency rather than building a complete multi-currency accounting system.

### Rate Limiting

Application-level rate limiting is not currently implemented.

### Pagination

List endpoints are intentionally kept simple for the current scope.

### PSP Reconciliation

Automatic reconciliation is not implemented because the Mock PSP does not provide a status-query API.

---

# 18. Scaling Considerations

The current design is appropriate for the assessment scope.

At approximately 100× the current workload, the following changes would become reasonable:

### Payment attempts

`payment_attempts` is append-heavy and can grow significantly.

A production system could partition it by time, for example monthly partitions.

### Webhook delivery

Webhook delivery should move from database polling toward a dedicated queue such as:

```text
Application
    │
    ▼
Message Queue
    │
    ▼
Webhook Workers
```

This allows worker capacity to scale independently.

### Read traffic

If invoice and customer reads become the primary database bottleneck, read replicas can be introduced.

The architecture would then separate:

```text
Writes ──► Primary PostgreSQL
Reads  ──► Read Replica
```

---

# 19. Production Readiness Gaps

The following areas would require additional work before treating the service as a production payment platform.

## Observability

Current logging provides application visibility, but production operation would benefit from:

* metrics
* request tracing
* payment latency metrics
* PSP latency/error metrics
* webhook delivery metrics
* structured correlation IDs
* alerting

## PSP Reconciliation

A production PSP integration should expose a status lookup API.

A reconciliation worker could then identify:

```text
pending payment
      │
      ▼
Older than threshold
      │
      ▼
Query PSP
      │
   ┌──┴────┐
   │       │
Success  Failed
   │       │
   ▼       ▼
Paid     Failed
```

## Rate Limiting

Payment and authentication-related endpoints should be protected against abuse.

Potential targets include:

```text
POST /api/businesses
POST /api/invoices/{id}/pay
```

---

# 20. Design Principles

The implementation follows several core principles:

### 1. Database is the source of truth

Invoice state and payment history are persisted in PostgreSQL.

### 2. Money uses integer cents

No floating-point arithmetic is used for monetary values.

### 3. Payment operations are idempotent

Retries should not unintentionally create duplicate PSP charges.

### 4. Concurrent payment requests are serialized

PostgreSQL row locking prevents multiple requests from successfully paying the same invoice.

### 5. External failures are explicit

PSP timeouts and failures result in deterministic payment-attempt states.

### 6. Webhooks are asynchronous

Webhook receivers cannot block the critical payment path.

### 7. Business isolation is enforced

Authenticated requests operate within the business associated with the API key.

### 8. Production limitations are explicit

The design intentionally documents the boundaries of the current implementation rather than pretending the Mock PSP provides guarantees that it does not.

---

# 21. Summary

The core design is centered around one invariant:

> **An invoice should have one authoritative financial state, and payment operations must remain correct even when requests are retried, executed concurrently, or fail at external boundaries.**

PostgreSQL transactions and row-level locking provide consistency inside the service.

Idempotency provides safe retries.

The payment-attempt table provides an audit trail.

The Mock PSP provides deterministic external failure scenarios.

Asynchronous webhook processing keeps notification delivery outside the critical payment path.

The remaining production gaps—particularly PSP reconciliation, rate limiting, and deeper observability—are deliberately identified rather than hidden.
