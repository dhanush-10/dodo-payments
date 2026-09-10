# Dodo Invoice Service

A minimal invoice and payment service for a billing product.

A business can:

* Create and manage customers
* Create invoices with line items
* Process invoice payments
* Prevent duplicate charges with idempotency keys
* Handle concurrent payment attempts safely
* Receive signed webhook events when invoice state changes
* Manage API keys and webhook endpoints

The service uses a **mock payment processor (PSP)** to simulate an external payment provider. The invoice service communicates with the PSP over HTTP, keeping the integration boundary similar to a real payment-provider integration.

See [`DESIGN.md`](DESIGN.md) for the design rationale, including the data model, invoice state machine, concurrency control, idempotency, PSP failure handling, webhook delivery, API-key model, and deliberate scope decisions.

See [`AI_USAGE.md`](AI_USAGE.md) for details about how AI tools were used during development.

---

## Demo Video

**Demo:** : https://uploadnow.io/files/N5VBzWg

The demo walks through the complete flow:

```text
Business
   ↓
Customer
   ↓
Invoice
   ↓
Webhook registration
   ↓
Customer pays invoice
   ↓
Invoice Service → Mock PSP
   ↓
Payment succeeds
   ↓
Invoice becomes PAID
   ↓
Signed webhook is delivered
```

---

## Architecture

The application runs as one process with two HTTP listeners:

```text
                    ┌─────────────────────────┐
                    │       Client            │
                    └────────────┬────────────┘
                                 │
                                 │ HTTP
                                 ▼
                    ┌─────────────────────────┐
                    │   Invoice API :3000     │
                    │                         │
                    │ Businesses              │
                    │ Customers               │
                    │ Invoices                │
                    │ Payments                │
                    │ Webhooks                │
                    └───────┬─────────┬───────┘
                            │         │
                 HTTP       │         │ SQL
                            │         ▼
                            │   ┌──────────────┐
                            │   │ PostgreSQL   │
                            │   │    :5432     │
                            │   └──────────────┘
                            │
                            ▼
                    ┌─────────────────────────┐
                    │     Mock PSP :3001      │
                    │                         │
                    │ success                 │
                    │ declined                │
                    │ insufficient funds      │
                    │ timeout                 │
                    │ network error           │
                    └─────────────────────────┘
```

The Mock PSP is intentionally separate from the invoice/payment logic. This allows payment success, decline, timeout, and network-error paths to be tested deterministically without requiring a real payment provider.

---

## Tech Stack

* **Rust**
* **Axum** — HTTP API
* **SQLx** — PostgreSQL access and migrations
* **PostgreSQL** — persistent storage
* **Tokio** — async runtime
* **Serde** — JSON serialization/deserialization
* **Tower HTTP** — request tracing
* **Docker Compose** — local development environment

---

## Run It

The easiest way to run the complete application is:

```bash
docker compose up --build
```

The application automatically runs database migrations during startup.

Once running:

| Service     | Address                 |
| ----------- | ----------------------- |
| Invoice API | `http://localhost:3000` |
| Mock PSP    | `http://localhost:3001` |
| PostgreSQL  | `localhost:5432`        |

Check that the Mock PSP is running:

```bash
curl http://localhost:3001/health
```

Expected:

```text
OK
```

---

# API Walkthrough

The following demonstrates the complete business flow.

## 1. Create a Business

A newly created business receives an API key.

> The API key is returned only when it is created. Save it for subsequent requests.

```bash
curl -X POST http://localhost:3000/api/businesses \
  -H "Content-Type: application/json" \
  -d "{\"name\":\"My Business\"}"
```

Example response:

```json
{
  "id": "business-uuid",
  "api_key": "dk_xxxxx_yyyyy"
}
```

Save:

```text
BUSINESS_API_KEY=dk_xxxxx_yyyyy
```

---

## 2. Create a Customer

```bash
curl -X POST http://localhost:3000/api/customers \
  -H "Authorization: Bearer dk_xxxxx_yyyyy" \
  -H "Content-Type: application/json" \
  -d "{\"name\":\"John Doe\",\"email\":\"john@example.com\"}"
```

Save the returned customer ID:

```text
CUSTOMER_ID=<returned-customer-id>
```

---

## 3. Register a Webhook

Register the webhook before creating or paying the invoice so that the complete event flow can be demonstrated.

For example, using a webhook inspection service:

```bash
curl -X POST http://localhost:3000/api/webhooks \
  -H "Authorization: Bearer dk_xxxxx_yyyyy" \
  -H "Content-Type: application/json" \
  -d "{\"url\":\"https://webhook.site/your-id-here\"}"
```

The service stores the webhook endpoint and uses it for subsequent event delivery.

---

## 4. Create an Invoice

The invoice total is calculated by the server from the supplied line items.

The client does **not** provide the final invoice total.

```bash
curl -X POST http://localhost:3000/api/invoices \
  -H "Authorization: Bearer dk_xxxxx_yyyyy" \
  -H "Content-Type: application/json" \
  -d "{
    \"customer_id\":\"<CUSTOMER_ID>\",
    \"due_date\":\"2026-12-31\",
    \"line_items\":[
      {
        \"description\":\"Consulting\",
        \"quantity\":5,
        \"unit_amount_cents\":10000
      }
    ]
  }"
```

The server calculates:

```text
5 × 10000 = 50000 cents
```

Save the returned invoice ID:

```text
INVOICE_ID=<returned-invoice-id>
```

---

## 5. Pay the Invoice — Successful Payment

Every payment attempt requires an `Idempotency-Key`.

```bash
curl -X POST http://localhost:3000/api/invoices/<INVOICE_ID>/pay \
  -H "Authorization: Bearer dk_xxxxx_yyyyy" \
  -H "Content-Type: application/json" \
  -H "Idempotency-Key: demo-payment-001" \
  -d "{\"card_token\":\"tok_success\"}"
```

The request flow is:

```text
Client
  │
  ▼
POST /api/invoices/:id/pay
  │
  ▼
Invoice Service
  │
  │ HTTP
  ▼
Mock PSP :3001
  │
  ▼
Payment succeeded
  │
  ▼
Invoice → PAID
  │
  ▼
Webhook event queued/delivered
```

The `tok_success` token causes the Mock PSP to return a successful payment response.

---

## 6. Verify the Invoice Status

Fetch the invoice:

```bash
curl http://localhost:3000/api/invoices/<INVOICE_ID> \
  -H "Authorization: Bearer dk_xxxxx_yyyyy"
```

The invoice should now show its successful/paid state.

This is the key result to show during the demo.

---

# Payment Failure Scenarios

The Mock PSP provides deterministic failure scenarios.

| Card Token               | Behavior                      |
| ------------------------ | ----------------------------- |
| `tok_success`            | Successful payment            |
| `tok_insufficient_funds` | Insufficient funds            |
| `tok_card_declined`      | Card declined                 |
| `tok_timeout`            | PSP responds after 30 seconds |
| `tok_network_error`      | PSP returns HTTP 500          |
| Any other token          | Invalid token                 |

For example:

```bash
curl -X POST http://localhost:3000/api/invoices/<INVOICE_ID>/pay \
  -H "Authorization: Bearer dk_xxxxx_yyyyy" \
  -H "Content-Type: application/json" \
  -H "Idempotency-Key: demo-payment-002" \
  -d "{\"card_token\":\"tok_card_declined\"}"
```

Use a separate unpaid invoice when demonstrating different payment outcomes.

---

# Webhooks

Webhook events notify the business when relevant invoice state changes occur.

The webhook system is designed around:

* Persisted webhook events
* Signed webhook payloads
* Asynchronous delivery
* Retry handling
* Delivery tracking
* Per-endpoint management

A typical flow is:

```text
Invoice state changes
        │
        ▼
Create webhook event
        │
        ▼
Persist event
        │
        ▼
Worker picks up event
        │
        ▼
Sign payload
        │
        ▼
HTTP POST to registered endpoint
        │
        ├── Success → mark delivered
        │
        └── Failure → retry
```

Webhook management endpoints:

```text
POST   /api/webhooks
GET    /api/webhooks
DELETE /api/webhooks/:id
```

---

# API Keys

Business APIs are authenticated using:

```http
Authorization: Bearer <api-key>
```

API keys can be:

* Created when a business is created
* Listed
* Revoked

Management endpoints:

```text
GET    /api/businesses/keys
DELETE /api/businesses/keys/:key_id
```

The raw API key is only returned when it is initially created.

---

# Idempotency

Payment requests require an `Idempotency-Key`.

Example:

```http
Idempotency-Key: payment-123
```

If the same payment request is retried with the same key, the service returns the previously stored result rather than creating another payment attempt.

This protects against duplicate charges caused by:

* Client retries
* Network failures
* Request timeouts
* Accidental duplicate submissions

---

# Concurrency

The payment flow protects an invoice from being paid multiple times concurrently.

Multiple simultaneous payment requests cannot transition the same invoice into a successful paid state more than once.

The integration tests specifically exercise concurrent payment attempts to verify this behavior.

---

# Testing

Run:

```bash
cargo test
```

The tests focus on important payment-system properties rather than handler-by-handler coverage.

### Concurrency

Multiple simultaneous payment attempts are made against the same invoice.

The test verifies that:

* Only one payment succeeds
* The invoice is not paid twice
* No duplicate charge is created

### Idempotency

The same payment request is retried with the same idempotency key.

The test verifies that:

* The same response is returned
* A second PSP call is not created

### PSP Failure

PSP timeout and network-error scenarios are exercised.

The test verifies that:

* The payment failure is handled
* The invoice is not left in an invalid state
* The system remains recoverable

---

# Other API Endpoints

### Businesses

```text
POST   /api/businesses
GET    /api/businesses/keys
DELETE /api/businesses/keys/:key_id
```

### Customers

```text
POST   /api/customers
GET    /api/customers
GET    /api/customers/:id
```

### Invoices

```text
POST   /api/invoices
GET    /api/invoices
GET    /api/invoices/:id
POST   /api/invoices/:id/pay
```

### Webhooks

```text
POST   /api/webhooks
GET    /api/webhooks
DELETE /api/webhooks/:id
```

Full request/response schemas and error formats are documented in [`openapi.yaml`](openapi.yaml).

---

# Environment Variables

| Variable       | Description                  | Default                 |
| -------------- | ---------------------------- | ----------------------- |
| `DATABASE_URL` | PostgreSQL connection string | —                       |
| `PSP_BASE_URL` | Mock PSP base URL            | `http://localhost:3001` |
| `PORT`         | Invoice API port             | `3000`                  |
| `PSP_PORT`     | Mock PSP port                | `3001`                  |
| `RUST_LOG`     | Logging level                | `info`                  |

Example:

```env
DATABASE_URL=postgres://postgres:postgres@localhost:5432/dodo_payments
PSP_BASE_URL=http://localhost:3001
PORT=3000
PSP_PORT=3001
RUST_LOG=info
```

---

# Project Structure

```text
dodo-payments/
├── migrations/
├── src/
│   ├── main.rs
│   ├── auth.rs
│   ├── error.rs
│   ├── models.rs
│   ├── mock_psp.rs
│   ├── psp_client.rs
│   ├── state_machine.rs
│   ├── worker.rs
│   └── handlers/
│       ├── businesses.rs
│       ├── customers.rs
│       ├── invoices.rs
│       ├── payments.rs
│       └── webhook_endpoints.rs
├── Cargo.toml
├── Cargo.lock
├── Dockerfile
├── docker-compose.yml
├── openapi.yaml
├── DESIGN.md
├── AI_USAGE.md
└── README.md
```

---

# Design Notes

The implementation intentionally focuses on the core billing and payment lifecycle.

The main design priorities are:

1. **Correctness over feature breadth**
2. **No duplicate successful payments**
3. **Idempotent payment retries**
4. **Explicit invoice state transitions**
5. **Deterministic PSP failure handling**
6. **Asynchronous webhook delivery**
7. **Signed webhook payloads**
8. **Simple API-key authentication**
9. **PostgreSQL-backed persistence**

For detailed reasoning and trade-offs, see [`DESIGN.md`](DESIGN.md).
