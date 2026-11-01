//! Pulls publicly listed LinkedIn job cards (guest search, no login) for
//! DevOps Engineer roles posted in the last 7 days, then loads each public
//! job page for the description and criteria.
//!
//! LinkedIn's guest endpoints are unofficial, rate-limited, and may change or
//! block automated clients. Their Terms of Service restrict scraping. Prefer
//! linkedin.com/jobs in a browser, or a partner Jobs API, for anything beyond
//! a small personal lookup. This tool does not log in, use cookies, or bypass
//! access controls.

mod cache;
mod ui;

use chrono::{Local, NaiveDate};
use clap::Parser;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, USER_AGENT};
use scraper::{ElementRef, Html, Selector};
use std::collections::HashSet;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "linkedin-devops-jobs",
    about = "List public LinkedIn DevOps Engineer jobs posted in the last 7 days",
    after_help = "\
A job id with a saved description is not fetched again. Startup opens rows
from the past 7 days (by the listed date) and only asks LinkedIn for ids that
are not already saved. Older rows stay in the SQLite file and are not shown.
A 429 stops new detail fetches and leaves the rows on disk; the next run
continues from them.
Earlier docs/jobs-*.json feeds and docs/job-cache.json in the same folder
are imported by id once, then those files are not used again.

The desk matches docs/jobs-board.json (tui-design): role list, pinned
facts, scrolling description. Keys: j/k role, J/K description, s save, q quit.
s writes the current cards into the SQLite file.

The description is the full guest job page, not the 700-character preview.

Search cards only have title, company, location, list date, and a url.
Description, seniority, employment type, industries, and applicant count
come from the public guest job page:
  https://www.linkedin.com/jobs-guest/jobs/api/jobPosting/<id>

Uses f_TPR=r604800 (past 7 days). No account, cookies, or login."
)]
struct Args {
    /// Search keywords.
    #[arg(short = 'k', long, default_value = "DevOps Engineer")]
    keywords: String,

    /// Search location. Pass an empty string to skip.
    #[arg(short = 'l', long, default_value = "Hong Kong")]
    location: String,

    /// Stop after this many guest pages (10 cards each).
    /// Default walks pages until the result set ends.
    #[arg(short = 'p', long, value_parser = clap::value_parser!(u32).range(1..=40))]
    pages: Option<u32>,

    /// Skip the public job page (no description).
    #[arg(long)]
    no_details: bool,

    /// Print cards instead of the job desk.
    #[arg(long)]
    plain: bool,

    /// SQLite file, keyed by job id. Table standard: docs/job-cache.sql.
    #[arg(long, default_value = cache::DEFAULT_PATH)]
    cache: String,

    /// Do not read or write the cache file.
    #[arg(long)]
    no_cache: bool,
}

const GUEST_SEARCH: &str =
    "https://www.linkedin.com/jobs-guest/jobs/api/seeMoreJobPostings/search";
const GUEST_JOB: &str = "https://www.linkedin.com/jobs-guest/jobs/api/jobPosting/";

/// LinkedIn `f_TPR=r604800` means "posted in the past 7 days".
const PAST_WEEK: &str = "r604800";
const PAGE_SIZE: u32 = 10;
const MAX_PAGES: u32 = 40;
const WRAP: usize = 88;

const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

#[derive(Clone)]
pub struct Job {
    id: String,
    title: String,
    company: String,
    location: String,
    keywords: String,
    search_location: String,
    listed: String,
    benefits: String,
    url: String,
    applicants: String,
    seniority: String,
    employment: String,
    industries: String,
    description: String,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let keywords = args.keywords;
    let location = args.location;
    let walk_all = args.pages.is_none();
    let pages = args.pages.unwrap_or(MAX_PAGES);
    let details = !args.no_details;
    let plain = args.plain;
    let cache_path = args.cache;
    let no_cache = args.no_cache;

    let client = match Client::builder().timeout(Duration::from_secs(20)).build() {
        Ok(c) => c,
        Err(err) => {
            eprintln!("failed to build HTTP client: {err}");
            return ExitCode::from(1);
        }
    };

    let today = Local::now().date_naive();
    let queried = Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string();
    let cache_file = std::path::PathBuf::from(&cache_path);
    let store = if no_cache {
        None
    } else {
        match cache::Cache::open(&cache_file) {
            Ok(store) => Some(store),
            Err(err) => {
                eprintln!("could not open cache {}: {err}", cache_file.display());
                return ExitCode::from(1);
            }
        }
    };
    let cached_at_start = store.as_ref().map(|store| store.len()).unwrap_or(0);
    if cached_at_start > 0 {
        eprintln!(
            "cache: {cached_at_start} job(s) already saved in {}",
            cache_file.display()
        );
    }
    if plain {
        println!("DevOps jobs posted in the last 7 days — queried {queried}");
        println!("keywords: {keywords}");
        if !location.is_empty() {
            println!("location: {location}");
        }
        println!();
    }

    let mut jobs_all = Vec::new();
    let mut seen_ids = HashSet::new();
    let mut cache_hits = 0usize;
    let mut fetched = 0usize;
    let mut rate_limited = false;
    if let Some(store) = store.as_ref() {
        match store.search(&keywords, &location) {
            Ok(saved) => {
                let mut stale = 0usize;
                for job in saved {
                    if job.id.is_empty() || !seen_ids.insert(job.id.clone()) {
                        continue;
                    }
                    // Keep the row so this id is not fetched again, but do not
                    // put a posting older than the guest 7-day window on the desk.
                    if !listed_in_past_week(&job.listed, today) {
                        stale += 1;
                        continue;
                    }
                    if plain {
                        print_job(jobs_all.len() + 1, &job);
                    }
                    jobs_all.push(job);
                    cache_hits += 1;
                }
                if stale > 0 {
                    eprintln!(
                        "cache: {stale} saved job(s) are older than 7 days; left on disk, not shown"
                    );
                }
            }
            Err(err) => {
                eprintln!("could not read cache: {err}");
                return ExitCode::from(1);
            }
        }
    }
    if !jobs_all.is_empty() {
        eprintln!(
            "cache: opened {} saved job(s) for this search; checking guest pages for new ids only",
            jobs_all.len()
        );
    }
    for page in 0..pages {
        if rate_limited {
            break;
        }
        let start = page * PAGE_SIZE;
        match fetch_page(&client, &keywords, &location, start) {
            Ok(html) if html.trim().is_empty() => break,
            Ok(html) => {
                let mut jobs = parse_jobs(&html);
                if jobs.is_empty() {
                    if page == 0 {
                        eprintln!(
                            "no job cards parsed. LinkedIn may have blocked the guest endpoint or changed the markup."
                        );
                        return ExitCode::from(1);
                    }
                    break;
                }
                eprint!("\r  guest page {}  ", page + 1);
                let _ = std::io::Write::flush(&mut std::io::stderr());
                let mut fresh = 0usize;
                for job in &mut jobs {
                    if !job.id.is_empty() && !seen_ids.insert(job.id.clone()) {
                        continue;
                    }
                    fresh += 1;
                    job.keywords = keywords.clone();
                    job.search_location = location.clone();
                    if details && job.id.chars().all(|c| c.is_ascii_digit()) && !job.id.is_empty() {
                        if let Some(hit) = store.as_ref().and_then(|store| store.get(&job.id)) {
                            cache::apply(job, &hit);
                            cache_hits += 1;
                        } else {
                            eprint!(
                                "\r  {}  {}  ",
                                jobs_all.len() + 1,
                                job.title.chars().take(48).collect::<String>()
                            );
                            match fetch_detail(&client, &job.id) {
                                Ok(body) => {
                                    fill_detail(job, &body);
                                    if cache::has_detail(job) {
                                        if let Some(store) = store.as_ref() {
                                            if let Err(err) = store.put(job) {
                                                eprintln!("\ncache write failed: {err}");
                                            } else {
                                                fetched += 1;
                                            }
                                        }
                                    }
                                }
                                Err(err) => {
                                    eprintln!("\ndetail failed (job {}): {err}", job.id);
                                    if err.status() == Some(reqwest::StatusCode::TOO_MANY_REQUESTS)
                                    {
                                        rate_limited = true;
                                        eprintln!(
                                            "429 from LinkedIn. Stopping new detail fetches. Cached ids stay on disk; rerun later for the rest."
                                        );
                                    }
                                }
                            }
                            thread::sleep(Duration::from_millis(400));
                        }
                    }
                    if plain {
                        print_job(jobs_all.len() + 1, job);
                    }
                    jobs_all.push(take_job(job));
                    if rate_limited {
                        break;
                    }
                }
                if fresh == 0 || (walk_all && jobs.len() < PAGE_SIZE as usize) {
                    break;
                }
            }
            Err(err) => {
                eprintln!("request failed (start={start}): {err}");
                if page == 0 && jobs_all.is_empty() {
                    return ExitCode::from(1);
                }
                break;
            }
        }
        if page + 1 < pages {
            thread::sleep(Duration::from_millis(800));
        }
    }

    eprintln!();
    if let Some(store) = store.as_ref() {
        if fetched > 0 || cache_hits > 0 || cached_at_start > 0 {
            eprintln!(
                "cache: {cache_hits} reused, {fetched} newly saved, {} total in {}",
                store.len(),
                cache_file.display()
            );
        }
    }
    if plain {
        if jobs_all.is_empty() {
            println!("No matching jobs returned.");
        } else {
            println!(
                "{} guest card(s) for this keyword in the past 7 days (f_TPR=r604800). Related titles are included.",
                jobs_all.len()
            );
        }
        return ExitCode::SUCCESS;
    }
    let desk_cache = if no_cache { "" } else { cache_path.as_str() };
    if let Err(err) = ui::run(&jobs_all, &keywords, &location, &queried, desk_cache) {
        eprintln!("desk failed: {err}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn print_job(n: usize, job: &Job) {
    println!("{n}. {}", job.title);
    println!("   company:     {}", empty(&job.company));
    println!("   location:    {}", empty(&job.location));
    if !job.listed.is_empty() {
        println!("   listed:      {}", job.listed);
    }
    if !job.applicants.is_empty() {
        println!("   applicants:  {}", job.applicants);
    }
    if !job.seniority.is_empty() {
        println!("   seniority:   {}", job.seniority);
    }
    if !job.employment.is_empty() {
        println!("   type:        {}", job.employment);
    }
    if !job.industries.is_empty() {
        println!("   industries:  {}", job.industries);
    }
    if !job.benefits.is_empty() {
        println!("   card note:   {}", job.benefits);
    }
    println!("   url:         {}", job.url);
    if !job.description.is_empty() {
        println!("   description:");
        for para in job.description.split("\n\n") {
            let para = para.trim();
            if para.is_empty() {
                continue;
            }
            for line in wrap_text(para, WRAP) {
                println!("     {line}");
            }
            println!();
        }
    } else if !job.url.is_empty() {
        println!("   description: (not loaded — open the url, or drop --no-details)");
        println!();
    }
    println!();
}

fn take_job(job: &mut Job) -> Job {
    Job {
        id: std::mem::take(&mut job.id),
        title: std::mem::take(&mut job.title),
        company: std::mem::take(&mut job.company),
        location: std::mem::take(&mut job.location),
        keywords: std::mem::take(&mut job.keywords),
        search_location: std::mem::take(&mut job.search_location),
        listed: std::mem::take(&mut job.listed),
        benefits: std::mem::take(&mut job.benefits),
        url: std::mem::take(&mut job.url),
        applicants: std::mem::take(&mut job.applicants),
        seniority: std::mem::take(&mut job.seniority),
        employment: std::mem::take(&mut job.employment),
        industries: std::mem::take(&mut job.industries),
        description: std::mem::take(&mut job.description),
    }
}

/// Guest search uses `f_TPR=r604800` (7 days). A missing date stays visible.
fn listed_in_past_week(listed: &str, today: NaiveDate) -> bool {
    let head = listed.split_whitespace().next().unwrap_or("");
    if head.len() < 10 {
        return true;
    }
    let Ok(date) = NaiveDate::parse_from_str(&head[..10], "%Y-%m-%d") else {
        return true;
    };
    let age = today.signed_duration_since(date).num_days();
    (0..=7).contains(&age)
}

fn empty(s: &str) -> &str {
    if s.is_empty() {
        "—"
    } else {
        s
    }
}

fn fetch_page(
    client: &Client,
    keywords: &str,
    location: &str,
    start: u32,
) -> Result<String, reqwest::Error> {
    let mut req = client
        .get(GUEST_SEARCH)
        .header(USER_AGENT, UA)
        .header(ACCEPT, "text/html")
        .header(ACCEPT_LANGUAGE, "en-US,en;q=0.9")
        .query(&[
            ("keywords", keywords),
            ("f_TPR", PAST_WEEK),
            ("start", &start.to_string()),
        ]);
    if !location.is_empty() {
        req = req.query(&[("location", location)]);
    }
    req.send()?.error_for_status()?.text()
}

fn fetch_detail(client: &Client, id: &str) -> Result<String, reqwest::Error> {
    client
        .get(format!("{GUEST_JOB}{id}"))
        .header(USER_AGENT, UA)
        .header(ACCEPT, "text/html")
        .header(ACCEPT_LANGUAGE, "en-US,en;q=0.9")
        .send()?
        .error_for_status()?
        .text()
}

fn parse_jobs(html: &str) -> Vec<Job> {
    let document = Html::parse_document(html);
    let card = Selector::parse("li").unwrap();
    let title_sel = Selector::parse("h3.base-search-card__title").unwrap();
    let company_sel = Selector::parse("h4.base-search-card__subtitle").unwrap();
    let location_sel = Selector::parse(".job-search-card__location").unwrap();
    let time_sel = Selector::parse("time").unwrap();
    let link_sel = Selector::parse("a.base-card__full-link").unwrap();
    let benefits_sel = Selector::parse(".job-posting-benefits__text").unwrap();
    let urn_sel = Selector::parse("[data-entity-urn]").unwrap();

    let mut jobs = Vec::new();
    for node in document.select(&card) {
        let title = text_of(&node, &title_sel);
        let url = node
            .select(&link_sel)
            .next()
            .and_then(|a| a.value().attr("href"))
            .unwrap_or("")
            .split('?')
            .next()
            .unwrap_or("")
            .to_string();
        if title.is_empty() || url.is_empty() || !url.contains("linkedin.com/") {
            continue;
        }
        let id = node
            .select(&urn_sel)
            .next()
            .and_then(|el| el.value().attr("data-entity-urn"))
            .and_then(|urn| urn.rsplit(':').next())
            .unwrap_or("")
            .to_string();
        let listed = node
            .select(&time_sel)
            .next()
            .map(|t| {
                let when = clean(&t.text().collect::<String>());
                match t.value().attr("datetime") {
                    Some(d) if !when.is_empty() => format!("{d} ({when})"),
                    Some(d) => d.to_string(),
                    None => when,
                }
            })
            .unwrap_or_default();
        jobs.push(Job {
            id,
            title,
            company: text_of(&node, &company_sel),
            location: text_of(&node, &location_sel),
            keywords: String::new(),
            search_location: String::new(),
            listed,
            benefits: text_of(&node, &benefits_sel),
            url,
            applicants: String::new(),
            seniority: String::new(),
            employment: String::new(),
            industries: String::new(),
            description: String::new(),
        });
    }
    jobs
}

fn fill_detail(job: &mut Job, html: &str) {
    let document = Html::parse_document(html);
    let applicants = Selector::parse(".num-applicants__caption").unwrap();
    let posted = Selector::parse(".posted-time-ago__text").unwrap();
    let item = Selector::parse(".description__job-criteria-item").unwrap();
    let header = Selector::parse(".description__job-criteria-subheader").unwrap();
    let value = Selector::parse(".description__job-criteria-text").unwrap();
    let desc = Selector::parse(".show-more-less-html__markup").unwrap();

    if let Some(el) = document.select(&applicants).next() {
        job.applicants = clean(&el.text().collect::<String>());
    }
    if let Some(el) = document.select(&posted).next() {
        let when = clean(&el.text().collect::<String>());
        if !when.is_empty() && !job.listed.contains(&when) {
            if job.listed.is_empty() {
                job.listed = when;
            } else {
                job.listed = format!("{} / {when}", job.listed);
            }
        }
    }
    for node in document.select(&item) {
        let key = text_of(&node, &header).to_ascii_lowercase();
        let val = text_of(&node, &value);
        if val.is_empty() {
            continue;
        }
        if key.contains("seniority") {
            job.seniority = val;
        } else if key.contains("employment") {
            job.employment = val;
        } else if key.contains("industr") {
            job.industries = val;
        }
    }
    if let Some(el) = document.select(&desc).next() {
        job.description = html_to_text(&el.inner_html());
    }
}

fn html_to_text(fragment: &str) -> String {
    let mut spaced = fragment
        .replace("<br", "\n<br")
        .replace("</p>", "\n\n</p>")
        .replace("</li>", "\n</li>")
        .replace("</h1>", "\n\n</h1>")
        .replace("</h2>", "\n\n</h2>")
        .replace("</h3>", "\n\n</h3>")
        .replace("</div>", "\n</div>");
    // List items read better as bullets once tags are stripped.
    spaced = spaced.replace("<li>", "<li>- ");
    let doc = Html::parse_fragment(&spaced);
    let raw = doc.root_element().text().collect::<String>();
    let mut paras = Vec::new();
    for block in raw.split("\n\n") {
        let line = block
            .lines()
            .map(|l| clean(l))
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if !line.is_empty() {
            paras.push(line);
        }
    }
    paras.join("\n\n")
}

fn text_of(node: &ElementRef, sel: &Selector) -> String {
    node.select(sel)
        .next()
        .map(|el| clean(&el.text().collect::<String>()))
        .unwrap_or_default()
}

fn clean(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn wrap_text(s: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for raw in s.split('\n') {
        let words: Vec<&str> = raw.split_whitespace().collect();
        if words.is_empty() {
            continue;
        }
        let mut cur = String::new();
        for word in words {
            let extra = if cur.is_empty() { 0 } else { 1 };
            if !cur.is_empty() && cur.chars().count() + extra + word.chars().count() > width {
                lines.push(cur);
                cur = String::new();
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(word);
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
    }
    lines
}

