-- 20261018120000_initial_schema.down
-- Reverses the initial schema, in reverse creation order. Every foreign key in
-- this schema is declared inline at table creation, so reverse creation order is
-- a valid drop order.
--
-- Per schema.md rule 4 this exists for local and test teardown. A deployed system
-- rolls back with a compensating forward migration instead.

DROP TABLE IF EXISTS idempotency_keys CASCADE;
DROP TABLE IF EXISTS audit_events CASCADE;
DROP TABLE IF EXISTS usage_records CASCADE;
DROP TABLE IF EXISTS guest_admissions CASCADE;
DROP TABLE IF EXISTS invitations CASCADE;
DROP TABLE IF EXISTS allocation_ports CASCADE;
DROP TABLE IF EXISTS relay_allocations CASCADE;
DROP TABLE IF EXISTS relay_nodes CASCADE;
DROP TABLE IF EXISTS tunnel_mappings CASCADE;
DROP TABLE IF EXISTS tunnels CASCADE;
DROP TABLE IF EXISTS auth_sessions CASCADE;
DROP TABLE IF EXISTS users CASCADE;
