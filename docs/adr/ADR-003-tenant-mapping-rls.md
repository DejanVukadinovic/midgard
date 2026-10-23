# ADR-003: Organization + product → internal tenant ID; PostgreSQL RLS

* Status: accepted
* Date: 2026-10-23

## Context
Customer organizations use several products; data must be isolated per organization *per product*.

## Decision
`tenants(zitadel_organization_id, application_id)` is unique and yields a UUID. Business tables carry only `tenant_id` (tenant implies application). Shared tables with PostgreSQL row-level security (`FORCE`), a transaction-local `app.tenant_id`, and a runtime role that neither owns tables nor has `BYPASSRLS`.

## Alternatives considered
Database per tenant (operationally heavy), schema per tenant (migration fan-out), application-level filtering only (one missed `WHERE` leaks data).

## Tradeoffs
RLS adds per-transaction setup and requires discipline (`begin_tenant_tx`); query planning on shared tables needs tenant-leading indexes.

## Consequences
Isolation is enforced twice: tenant derived from verified claims, and the database refuses cross-tenant access even if application code is wrong. Tests prove both (`tenancy.rs`, example service tests).
