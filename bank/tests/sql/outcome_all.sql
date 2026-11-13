-- Inspect every persisted event for secret leakage and completion payload shape.
SELECT event_json FROM outbox
