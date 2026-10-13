ALTER TABLE routing_evidence ADD COLUMN pass_boot_lower REAL NOT NULL DEFAULT 0;
ALTER TABLE routing_evidence ADD COLUMN pass_boot_upper REAL NOT NULL DEFAULT 1;
