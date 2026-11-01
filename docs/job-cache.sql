-- Local job store. Version 1.0.
-- Standard: urn:linkedin-devops-jobs:job-cache:1.0
-- One row per LinkedIn job id. A non-empty description means the guest
-- detail page was already saved and must not be fetched again.

CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS jobs (
    id TEXT PRIMARY KEY,          -- LinkedIn job id, digits only
    title TEXT NOT NULL,
    company TEXT NOT NULL,
    location TEXT NOT NULL,
    keywords TEXT NOT NULL DEFAULT '',         -- guest search that found this id
    search_location TEXT NOT NULL DEFAULT '',  -- guest search location filter
    listed TEXT NOT NULL,         -- "YYYY-MM-DD (relative phrase)" when known
    benefits TEXT NOT NULL,
    url TEXT NOT NULL,
    applicants TEXT NOT NULL,
    seniority TEXT NOT NULL,
    employment TEXT NOT NULL,
    industries TEXT NOT NULL,
    description TEXT NOT NULL,    -- full guest description; empty rows are not stored
    saved_at TEXT NOT NULL        -- UTC ISO-8601
);
