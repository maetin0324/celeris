-- ADR-0118 D3: NULL means no reviewed merge candidate exists for this delivery.
ALTER TABLE deliveries ADD COLUMN target_sha TEXT;
ALTER TABLE deliveries ADD COLUMN reviewed_sha TEXT;
ALTER TABLE deliveries ADD COLUMN merge_candidate_sha TEXT;
