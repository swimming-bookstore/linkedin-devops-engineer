//! Local progress in SQLite, keyed by LinkedIn job id.
//!
//! A guest detail page is fetched once. Later runs reuse docs/job-cache.sqlite
//! so a 429 does not make us ask for the same id again. Existing
//! docs/jobs-*.json feeds and docs/job-cache.json are imported once.

use crate::Job;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::fs;
use std::path::Path;

pub const SCHEMA_ID: &str = "urn:linkedin-devops-jobs:job-cache:1.0";
pub const DEFAULT_PATH: &str = "docs/job-cache.sqlite";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS jobs (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    company TEXT NOT NULL,
    location TEXT NOT NULL,
    keywords TEXT NOT NULL DEFAULT '',
    search_location TEXT NOT NULL DEFAULT '',
    listed TEXT NOT NULL,
    benefits TEXT NOT NULL,
    url TEXT NOT NULL,
    applicants TEXT NOT NULL,
    seniority TEXT NOT NULL,
    employment TEXT NOT NULL,
    industries TEXT NOT NULL,
    description TEXT NOT NULL,
    saved_at TEXT NOT NULL
);
";

pub struct Cache {
    conn: Connection,
}

impl Cache {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        ensure_column(&conn, "keywords")?;
        ensure_column(&conn, "search_location")?;
        conn.execute(
            "INSERT OR IGNORE INTO meta (key, value) VALUES ('schema', ?1)",
            [SCHEMA_ID],
        )
        .map_err(|e| e.to_string())?;
        let cache = Self { conn };
        cache.import_nearby_json(path)?;
        Ok(cache)
    }

    pub fn len(&self) -> usize {
        self.conn
            .query_row("SELECT COUNT(*) FROM jobs", [], |row| row.get(0))
            .unwrap_or(0)
    }

    /// Saved rows for this guest search, newest listed day first.
    /// Rows written before the search columns existed have empty keywords and
    /// are included so an existing cache still opens without a refetch.
    pub fn search(&self, keywords: &str, location: &str) -> Result<Vec<Job>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, title, company, location, listed, benefits, url,
                        applicants, seniority, employment, industries, description
                 FROM jobs
                 WHERE keywords = ?1 OR keywords = ''
                 ORDER BY substr(listed, 1, 10) DESC, id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![keywords], |row| row_to_job(row))
            .map_err(|e| e.to_string())?;
        let mut jobs = Vec::new();
        for row in rows {
            let job = row.map_err(|e| e.to_string())?;
            if !location.is_empty() && !job.location.is_empty() && !location_matches(&job.location, location)
            {
                continue;
            }
            jobs.push(job);
        }
        Ok(jobs)
    }

    pub fn get(&self, id: &str) -> Option<Job> {
        self.conn
            .query_row(
                "SELECT id, title, company, location, listed, benefits, url,
                        applicants, seniority, employment, industries, description
                 FROM jobs WHERE id = ?1",
                [id],
                row_to_job,
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn put(&self, job: &Job) -> Result<(), String> {
        if !has_detail(job) {
            return Ok(());
        }
        self.upsert(job, true)
    }

    fn upsert(&self, job: &Job, replace: bool) -> Result<(), String> {
        let sql = if replace {
            "INSERT INTO jobs (
                id, title, company, location, keywords, search_location, listed, benefits, url,
                applicants, seniority, employment, industries, description, saved_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                company = excluded.company,
                location = excluded.location,
                keywords = CASE WHEN excluded.keywords = '' THEN keywords ELSE excluded.keywords END,
                search_location = CASE WHEN excluded.search_location = '' THEN search_location ELSE excluded.search_location END,
                listed = excluded.listed,
                benefits = excluded.benefits,
                url = excluded.url,
                applicants = excluded.applicants,
                seniority = excluded.seniority,
                employment = excluded.employment,
                industries = excluded.industries,
                description = excluded.description,
                saved_at = excluded.saved_at"
        } else {
            "INSERT OR IGNORE INTO jobs (
                id, title, company, location, keywords, search_location, listed, benefits, url,
                applicants, seniority, employment, industries, description, saved_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"
        };
        self.conn
            .execute(
                sql,
                params![
                    job.id,
                    job.title,
                    job.company,
                    job.location,
                    job.keywords,
                    job.search_location,
                    job.listed,
                    job.benefits,
                    job.url,
                    job.applicants,
                    job.seniority,
                    job.employment,
                    job.industries,
                    job.description,
                    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                ],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn import_nearby_json(&self, path: &Path) -> Result<(), String> {
        let Some(dir) = path.parent() else {
            return Ok(());
        };
        if dir.as_os_str().is_empty() {
            return Ok(());
        }
        let mut files = Vec::new();
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("jobs-") && name.ends_with(".json") {
                    files.push(entry.path());
                }
            }
        }
        let legacy = dir.join("job-cache.json");
        if legacy.exists() {
            files.push(legacy);
        }
        let mut imported = 0usize;
        let mut consumed = Vec::new();
        for file in &files {
            let n = self.import_file(file)?;
            if n > 0 {
                imported += n;
                consumed.push(file);
            }
        }
        if imported > 0 {
            eprintln!("cache: imported {imported} job(s) from nearby JSON into SQLite");
            // Only drop files that actually contributed rows. A parse miss
            // must not delete a feed that has not been imported yet.
            for file in consumed {
                if let Err(err) = fs::remove_file(file) {
                    eprintln!("could not remove {}: {err}", file.display());
                }
            }
        }
        Ok(())
    }

    fn import_file(&self, path: &Path) -> Result<usize, String> {
        let Ok(raw) = fs::read_to_string(path) else {
            return Ok(0);
        };
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            return Ok(0);
        };
        let Some(list) = value.get("jobs") else {
            return Ok(0);
        };
        let mut n = 0usize;
        match list {
            Value::Array(items) => {
                for item in items {
                    if self.insert_imported(item)? {
                        n += 1;
                    }
                }
            }
            Value::Object(map) => {
                for item in map.values() {
                    if self.insert_imported(item)? {
                        n += 1;
                    }
                }
            }
            _ => {}
        }
        Ok(n)
    }

    fn insert_imported(&self, value: &Value) -> Result<bool, String> {
        let Some(job) = job_from_value(value) else {
            return Ok(false);
        };
        let before = self.get(&job.id).is_some();
        self.upsert(&job, false)?;
        Ok(!before && self.get(&job.id).is_some())
    }
}

fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
    Ok(Job {
        id: row.get(0)?,
        title: row.get(1)?,
        company: row.get(2)?,
        location: row.get(3)?,
        listed: row.get(4)?,
        benefits: row.get(5)?,
        url: row.get(6)?,
        applicants: row.get(7)?,
        seniority: row.get(8)?,
        employment: row.get(9)?,
        industries: row.get(10)?,
        description: row.get(11)?,
        keywords: String::new(),
        search_location: String::new(),
    })
}

fn ensure_column(conn: &Connection, name: &str) -> Result<(), String> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(jobs)")
        .map_err(|e| e.to_string())?;
    let names: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if names.iter().any(|n| n == name) {
        return Ok(());
    }
    conn.execute(
        &format!("ALTER TABLE jobs ADD COLUMN {name} TEXT NOT NULL DEFAULT ''"),
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn location_matches(job_location: &str, filter: &str) -> bool {
    let job = job_location.to_ascii_lowercase();
    let filter = filter.to_ascii_lowercase();
    job.contains(filter.trim())
}

pub fn has_detail(job: &Job) -> bool {
    !job.id.is_empty() && !job.description.trim().is_empty()
}

pub fn apply(job: &mut Job, cached: &Job) {
    if job.title.is_empty() {
        job.title = cached.title.clone();
    }
    if job.company.is_empty() {
        job.company = cached.company.clone();
    }
    if job.location.is_empty() {
        job.location = cached.location.clone();
    }
    if job.url.is_empty() {
        job.url = cached.url.clone();
    }
    if job.benefits.is_empty() {
        job.benefits = cached.benefits.clone();
    }
    if job.listed.is_empty() {
        job.listed = cached.listed.clone();
    }
    job.applicants = cached.applicants.clone();
    job.seniority = cached.seniority.clone();
    job.employment = cached.employment.clone();
    job.industries = cached.industries.clone();
    job.description = cached.description.clone();
}

fn job_from_value(value: &Value) -> Option<Job> {
    let id = value.get("identifier")?.as_str()?.trim();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let description = text_field(value, "description");
    if description.is_empty() {
        return None;
    }
    Some(Job {
        id: id.to_string(),
        title: text_field(value, "title"),
        company: nested_name(value, "hiringOrganization")
            .unwrap_or_else(|| text_field(value, "company")),
        location: nested_name(value, "jobLocation").unwrap_or_else(|| text_field(value, "location")),
        keywords: String::new(),
        search_location: String::new(),
        listed: listed_text(value),
        benefits: text_field(value, "jobBenefits").if_empty(text_field(value, "benefits")),
        url: text_field(value, "url"),
        applicants: prop_value(value, "applicantNote").if_empty(text_field(value, "applicants")),
        seniority: text_field(value, "experienceRequirements").if_empty(text_field(value, "seniority")),
        employment: employment_label(value).if_empty(text_field(value, "employment")),
        industries: text_field(value, "industry").if_empty(text_field(value, "industries")),
        description,
    })
}

fn nested_name(value: &Value, key: &str) -> Option<String> {
    let name = value.get(key)?.get("name")?.as_str()?.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn text_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn prop_value(value: &Value, name: &str) -> String {
    let Some(props) = value.get("additionalProperty").and_then(|v| v.as_array()) else {
        return String::new();
    };
    for prop in props {
        if prop.get("name").and_then(|n| n.as_str()) == Some(name) {
            return prop
                .get("value")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
        }
    }
    String::new()
}

fn listed_text(value: &Value) -> String {
    let listed = text_field(value, "listed");
    if !listed.is_empty() {
        return listed;
    }
    let date = text_field(value, "datePosted");
    let phrase = prop_value(value, "listedText");
    match (date.is_empty(), phrase.is_empty()) {
        (true, true) => String::new(),
        (false, true) => date,
        (true, false) => phrase,
        (false, false) => format!("{date} ({phrase})"),
    }
}

fn employment_label(value: &Value) -> String {
    let raw = text_field(value, "employmentType");
    match raw.as_str() {
        "FULL_TIME" => "Full-time".into(),
        "PART_TIME" => "Part-time".into(),
        "CONTRACTOR" => "Contract".into(),
        "TEMPORARY" => "Temporary".into(),
        "INTERN" => "Internship".into(),
        "VOLUNTEER" => "Volunteer".into(),
        "PER_DIEM" => "Per diem".into(),
        "OTHER" => "Other".into(),
        _ => raw,
    }
}

trait IfEmpty {
    fn if_empty(self, fallback: String) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: String) -> String {
        if self.is_empty() {
            fallback
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Job {
        Job {
            id: "4470917265".into(),
            title: "Senior Staff, Platform Engineer, DevOps".into(),
            company: "MoneyHero".into(),
            location: "Hong Kong SAR".into(),
            keywords: String::new(),
            search_location: String::new(),
            listed: "2026-10-30 (3 hours ago)".into(),
            benefits: String::new(),
            url: "https://hk.linkedin.com/jobs/view/example".into(),
            applicants: "32 applicants".into(),
            seniority: "Mid-Senior level".into(),
            employment: "Full-time".into(),
            industries: "Financial Services".into(),
            description: "Own the platform.".into(),
        }
    }

    #[test]
    fn roundtrip_keeps_detail_by_id() {
        let dir = std::env::temp_dir().join(format!("job-cache-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("job-cache.sqlite");
        let cache = Cache::open(&path).unwrap();
        cache.put(&sample()).unwrap();
        let job = cache.get("4470917265").unwrap();
        assert_eq!(job.description, "Own the platform.");
        assert_eq!(job.employment, "Full-time");
        assert_eq!(cache.len(), 1);
        let again = Cache::open(&path).unwrap();
        assert_eq!(again.get("4470917265").unwrap().title, sample().title);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn json_feed_imports_once_by_id() {
        let dir = std::env::temp_dir().join(format!("job-cache-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("jobs-sample.json"),
            r#"{
                "jobs": [{
                    "identifier": "99",
                    "title": "DevOps Engineer",
                    "url": "https://example.test/99",
                    "datePosted": "2026-10-30",
                    "hiringOrganization": {"name": "Acme"},
                    "jobLocation": {"name": "Hong Kong SAR"},
                    "employmentType": "FULL_TIME",
                    "industry": "IT",
                    "description": "Run the pipeline.",
                    "experienceRequirements": "Mid-Senior level",
                    "additionalProperty": [
                        {"name": "listedText", "value": "1 hour ago"},
                        {"name": "applicantNote", "value": "4 applicants"}
                    ]
                }]
            }"#,
        )
        .unwrap();
        let cache = Cache::open(&dir.join("job-cache.sqlite")).unwrap();
        let job = cache.get("99").unwrap();
        assert_eq!(job.company, "Acme");
        assert_eq!(job.listed, "2026-10-30 (1 hour ago)");
        assert_eq!(job.employment, "Full-time");
        assert_eq!(job.applicants, "4 applicants");
        let again = Cache::open(&dir.join("job-cache.sqlite")).unwrap();
        assert_eq!(again.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_returns_saved_rows_without_a_network_walk() {
        let dir = std::env::temp_dir().join(format!("job-cache-search-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let cache = Cache::open(&dir.join("job-cache.sqlite")).unwrap();
        let mut kept = sample();
        kept.keywords = "DevOps Engineer".into();
        kept.search_location = "Hong Kong".into();
        cache.put(&kept).unwrap();
        let mut other = sample();
        other.id = "100".into();
        other.keywords = "Designer".into();
        other.search_location = "Singapore".into();
        other.location = "Singapore".into();
        cache.put(&other).unwrap();
        let found = cache.search("DevOps Engineer", "Hong Kong").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, kept.id);
        let _ = fs::remove_dir_all(&dir);
    }
}
