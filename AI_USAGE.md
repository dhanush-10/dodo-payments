# AI Usage Documentation

## AI Tools Used

- **Claude (Anthropic)**: Used for initial project structure scaffolding, generating boilerplate handlers, database schema drafting, and test file creation.
- **Cursor autocomplete**: Used for filling in repetitive code patterns like handler function signatures, error handling boilerplate, and SQL query building.

## Three Decisions I Made Independently

1. **State machine design**: AI proposed a simpler state flow with `draft → open → paid` only. I expanded to include `void` and `uncollectible` states to better handle failure scenarios and allow businesses to manually mark invoices as uncollectible after repeated payment failures. This gives more flexibility in real-world usage.

2. **Idempotency storage approach**: AI suggested using Redis for idempotency key caching. I chose PostgreSQL with unique constraint `(invoice_id, idempotency_key)` instead. This eliminates a dependency, ensures strong consistency with the payment attempt record, and simplifies deployment.

3. **Concurrency mechanism**: AI proposed optimistic locking with version checks. I used `SELECT ... FOR UPDATE` row-level locking because it's simpler to reason about, prevents deadlocks in our simple workflow, and guarantees exactly one payment succeeds without needing retry logic on the application side.

## One Thing AI Got Wrong

The AI initially generated the mock PSP as part of the same port as the main service using route prefix `/psp`. This would work but makes the mock PSP harder to run independently and test in isolation. I moved it to a separate binary (`src/bin/mock_psp.rs`) running on port 3001 with its own health check. This matches the architecture described in the assessment where the PSP is a separate service the invoice service calls over HTTP.

I verified correctness by running the full Docker Compose setup and confirming both services start independently and communicate correctly.