-- Migration: Add latitude and longitude to app_users
-- Run this against your PostgreSQL database before deploying the updated Rust service.

ALTER TABLE app_users
    ADD COLUMN IF NOT EXISTS latitude  DOUBLE PRECISION NULL,
    ADD COLUMN IF NOT EXISTS longitude DOUBLE PRECISION NULL;