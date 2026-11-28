//! `--parent` lookup across page types (`pages::resolve_parent`), and
//! `market-reports create` refusing an empty search, against a mock site.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use assert_cmd::assert::Assert;
use assert_cmd::Command;
use mockito::{Matcher, Mock, Server, ServerGuard};
use predicates::prelude::*;
use serde_json::json;

/// The order `resolve_parent` searches after the command's own type.
const PAGES: &str = "pages";
const AREA: &str = "area-pages";
const AGENT: &str = "agent-pages";
const REPORT: &str = "market-report-pages";
const VALUATION: &str = "valuation-pages";

struct Env {
    server: ServerGuard,
    config_dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            server: Server::new(),
            config_dir: tempfile::tempdir().unwrap(),
        }
    }

    fn cmd(&self) -> Command {
        let mut cmd = Command::cargo_bin("geekcli").unwrap();
        cmd.env_clear()
            .env(
                "SYSTEMROOT",
                std::env::var("SYSTEMROOT").unwrap_or_default(),
            )
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("GEEKCLI_SITE", "www.test.local")
            .env("GEEKCLI_API_KEY", "rg_live_k")
            .env("GEEKCLI_BASE_URL", self.server.url())
            .env("GEEKCLI_CONFIG_DIR", self.config_dir.path());
        cmd
    }

    /// `GET /api/v3/content/<kind>/` looked up by `reference` (by `path`
    /// when it starts with `/`, else by `slug`), answering `status` with `body`.
    fn lookup(&mut self, kind: &str, reference: &str, status: usize, body: &str) -> Mock {
        let key = if reference.starts_with('/') {
            "path"
        } else {
            "slug"
        };
        self.server
            .mock("GET", format!("/api/v3/content/{kind}/").as_str())
            .match_query(Matcher::UrlEncoded(key.into(), reference.into()))
            .with_status(status)
            .with_body(body)
            .create()
    }

    /// The lookup finds nothing on each of `kinds`.
    fn misses(&mut self, kinds: &[&str], reference: &str) -> Vec<Mock> {
        kinds
            .iter()
            .map(|kind| self.lookup(kind, reference, 200, r#"{"results":[],"pagination":{}}"#))
            .collect()
    }

    /// The lookup finds the page `id` on `kind`.
    fn hit(&mut self, kind: &str, reference: &str, id: u64) -> Mock {
        let path = if reference.starts_with('/') {
            reference.to_string()
        } else {
            format!("/x{id}/{reference}/")
        };
        let body = json!({"results": [{"id": id, "path": path}], "pagination": {}});
        self.lookup(kind, reference, 200, &body.to_string())
    }

    /// The lookup on `kind` fails with `status` and the API error `code`.
    fn fails(&mut self, kind: &str, reference: &str, status: usize, code: &str) -> Mock {
        let body = json!({"error": {"code": code, "message": "mock error"}});
        self.lookup(kind, reference, status, &body.to_string())
    }

    /// Any `method` request to `kind`; tests assert it never happens.
    fn never(&mut self, method: &str, kind: &str) -> Mock {
        self.server
            .mock(method, format!("/api/v3/content/{kind}/").as_str())
            // Without a query matcher a request with a query string would not
            // match, get mockito's 501, and be skipped as a 5xx unnoticed.
            .match_query(Matcher::Any)
            .with_status(201)
            .with_body(r#"{"id":100}"#)
            .expect(0)
            .create()
    }

    /// The create, which must send `parent: id`.
    fn created_under(&mut self, kind: &str, id: u64) -> Mock {
        self.server
            .mock("POST", format!("/api/v3/content/{kind}/").as_str())
            .match_body(Matcher::PartialJson(json!({ "parent": id })))
            .with_status(201)
            .with_body(r#"{"id":100}"#)
            .create()
    }

    /// `pages create --slug child --anchor-text Child --parent <parent> <extra>`.
    fn create_page(&self, parent: &str, extra: &[&str]) -> Assert {
        self.cmd()
            .args([
                "pages",
                "create",
                "--slug",
                "child",
                "--anchor-text",
                "Child",
                "--parent",
                parent,
            ])
            .args(extra)
            .assert()
    }
}

fn assert_all(mocks: &[Mock]) {
    for mock in mocks {
        mock.assert();
    }
}

// Found on another page type.

#[test]
fn market_reports_create_resolves_parent_from_area_pages() {
    let mut env = Env::new();
    let misses = env.misses(&[REPORT, PAGES], "/riverside/");
    let hit = env.hit(AREA, "/riverside/", 42);
    let post = env.created_under(REPORT, 42);

    env.cmd()
        .args([
            "market-reports",
            "create",
            "--slug",
            "q1",
            "--anchor-text",
            "Q1 Report",
            "--search",
            "12",
            "--parent",
            "/riverside/",
        ])
        .assert()
        .success();

    assert_all(&misses);
    hit.assert();
    post.assert();
}

#[test]
fn pages_create_resolves_parent_from_agent_pages() {
    let mut env = Env::new();
    let misses = env.misses(&[PAGES, AREA], "/agent-jane/");
    let hit = env.hit(AGENT, "/agent-jane/", 77);
    let post = env.created_under(PAGES, 77);

    env.create_page("/agent-jane/", &[]).success();

    assert_all(&misses);
    hit.assert();
    post.assert();
}

#[test]
fn pages_create_resolves_parent_from_valuation_pages() {
    let mut env = Env::new();
    let misses = env.misses(&[PAGES, AREA, AGENT, REPORT], "/home-worth/");
    let hit = env.hit(VALUATION, "/home-worth/", 55);
    let post = env.created_under(PAGES, 55);

    env.create_page("/home-worth/", &[]).success();

    assert_all(&misses);
    hit.assert();
    post.assert();
}

#[test]
fn resolve_parent_all_not_found_exits_4() {
    let mut env = Env::new();
    let misses = env.misses(&[PAGES, AREA, AGENT, REPORT, VALUATION], "/missing/");
    let post = env.never("POST", PAGES);

    env.create_page("/missing/", &[])
        .code(4)
        .stderr(predicate::str::contains("no parent page"));

    assert_all(&misses);
    post.assert();
}

// Ambiguous slugs.

#[test]
fn resolve_parent_ambiguous_slug_on_primary_exits_2_without_fallback() {
    let mut env = Env::new();
    let primary = env.lookup(
        PAGES,
        "shared",
        200,
        r#"{"results":[{"id":1,"path":"/a/shared/"},{"id":2,"path":"/b/shared/"}],"pagination":{}}"#,
    );
    // A unique match on another type must not win over ambiguity on the
    // command's own type.
    let fallback = env.never("GET", AREA);
    let post = env.never("POST", PAGES);

    env.create_page("shared", &[])
        .code(2)
        .stderr(predicate::str::contains("more than one parent page"));

    primary.assert();
    fallback.assert();
    post.assert();
}

#[test]
fn resolve_parent_ambiguous_slug_on_fallback_propagates_exit_2() {
    let mut env = Env::new();
    let primary = env.misses(&[PAGES], "shared-area");
    let fallback = env.lookup(
        AREA,
        "shared-area",
        200,
        r#"{"results":[{"id":10,"path":"/a/shared-area/"},{"id":11,"path":"/b/shared-area/"}],"pagination":{}}"#,
    );
    let next = env.never("GET", AGENT);
    let post = env.never("POST", PAGES);

    env.create_page("shared-area", &[])
        .code(2)
        .stderr(predicate::str::contains("more than one parent page"));

    assert_all(&primary);
    fallback.assert();
    next.assert();
    post.assert();
}

#[test]
fn resolve_parent_slug_found_in_two_fallback_types_is_ambiguous() {
    let mut env = Env::new();
    let misses = env.misses(&[PAGES, REPORT, VALUATION], "downtown");
    let area = env.hit(AREA, "downtown", 10);
    let agent = env.hit(AGENT, "downtown", 20);
    let post = env.never("POST", PAGES);

    // Slugs are unique only within one page type: two types matching is a
    // usage error, not "whichever type is searched first".
    env.create_page("downtown", &[])
        .code(2)
        .stderr(predicate::str::contains("more than one parent page"));

    assert_all(&misses);
    area.assert();
    agent.assert();
    post.assert();
}

#[test]
fn resolve_parent_slug_on_the_commands_own_type_wins_without_searching_others() {
    let mut env = Env::new();
    let own = env.hit(PAGES, "about", 1);
    // Another type with the same slug is not consulted, so it is not ambiguous.
    let other = env.never("GET", AREA);
    let post = env.created_under(PAGES, 1);

    env.create_page("about", &[]).success();

    own.assert();
    other.assert();
    post.assert();
}

// Errors on a page type: skipped, or stop the lookup.

#[test]
fn pages_create_resolves_parent_past_403_on_area_pages() {
    let mut env = Env::new();
    let primary = env.misses(&[PAGES], "/agent-jane/");
    let forbidden = env.fails(AREA, "/agent-jane/", 403, "forbidden");
    let hit = env.hit(AGENT, "/agent-jane/", 77);
    let post = env.created_under(PAGES, 77);

    env.create_page("/agent-jane/", &[]).success();

    assert_all(&primary);
    forbidden.assert();
    hit.assert();
    post.assert();
}

#[test]
fn resolve_parent_403_on_fallback_exits_4_with_skipped_note() {
    let mut env = Env::new();
    let primary = env.misses(&[PAGES], "/restricted/");
    let forbidden = env.fails(AREA, "/restricted/", 403, "forbidden");
    let rest = env.misses(&[AGENT, REPORT, VALUATION], "/restricted/");
    let post = env.never("POST", PAGES);

    env.create_page("/restricted/", &[]).code(4).stderr(
        predicate::str::contains("no parent page")
            .and(predicate::str::contains("content/area-pages/"))
            .and(predicate::str::contains("403")),
    );

    assert_all(&primary);
    forbidden.assert();
    assert_all(&rest);
    post.assert();
}

#[test]
fn resolve_parent_403_on_primary_aborts_immediately() {
    let mut env = Env::new();
    let primary = env.fails(PAGES, "/restricted/", 403, "forbidden");
    let fallback = env.never("GET", AREA);
    let post = env.never("POST", PAGES);

    env.create_page("/restricted/", &[])
        .code(3)
        .stderr(predicate::str::contains("forbidden"));

    primary.assert();
    fallback.assert();
    post.assert();
}

#[test]
fn resolve_parent_api_disabled_on_fallback_aborts_immediately() {
    let mut env = Env::new();
    let primary = env.misses(&[PAGES], "/disabled/");
    let disabled = env.fails(AREA, "/disabled/", 403, "api_disabled");
    let next = env.never("GET", AGENT);
    let post = env.never("POST", PAGES);

    env.create_page("/disabled/", &[])
        .code(3)
        .stderr(predicate::str::contains("api_disabled"));

    assert_all(&primary);
    disabled.assert();
    next.assert();
    post.assert();
}

#[test]
fn resolve_parent_401_on_fallback_aborts_immediately() {
    let mut env = Env::new();
    let primary = env.misses(&[PAGES], "/unauth/");
    let unauthorized = env.fails(AREA, "/unauth/", 401, "invalid_token");
    let next = env.never("GET", AGENT);
    let post = env.never("POST", PAGES);

    env.create_page("/unauth/", &[])
        .code(3)
        .stderr(predicate::str::contains("invalid_token"));

    assert_all(&primary);
    unauthorized.assert();
    next.assert();
    post.assert();
}

#[test]
fn resolve_parent_continues_past_502_or_503_on_fallback_and_finds_later_parent() {
    let mut env = Env::new();
    let misses = env.misses(&[PAGES, AREA], "/reports/");
    let unavailable = env.fails(AGENT, "/reports/", 502, "crm_unavailable");
    let hit = env.hit(REPORT, "/reports/", 55);
    let post = env.created_under(PAGES, 55);

    env.create_page("/reports/", &[]).success();

    assert_all(&misses);
    unavailable.assert();
    hit.assert();
    post.assert();
}

#[test]
fn resolve_parent_continues_past_429_on_fallback() {
    let mut env = Env::new();
    let misses = env.misses(&[PAGES, AREA], "/homes/");
    let limited = env
        .server
        .mock("GET", "/api/v3/content/agent-pages/")
        .match_query(Matcher::UrlEncoded("path".into(), "/homes/".into()))
        .with_status(429)
        .with_header("retry-after", "0")
        .with_body(r#"{"error":{"code":"rate_limited","message":"slow down"}}"#)
        .create();
    let hit = env.hit(REPORT, "/homes/", 55);
    let post = env.created_under(PAGES, 55);

    env.create_page("/homes/", &["--max-retries", "0"])
        .success();

    assert_all(&misses);
    limited.assert();
    hit.assert();
    post.assert();
}

#[test]
fn resolve_parent_continues_past_valuation_disabled_on_typo_and_exits_4() {
    let mut env = Env::new();
    let misses = env.misses(&[PAGES, AREA, AGENT, REPORT], "/typo/");
    let disabled = env.fails(VALUATION, "/typo/", 409, "valuation_disabled");
    let post = env.never("POST", PAGES);

    env.create_page("/typo/", &[]).code(4).stderr(
        predicate::str::contains("no parent page")
            .and(predicate::str::contains("content/valuation-pages/"))
            .and(predicate::str::contains("409"))
            .and(predicate::str::contains("valuation_disabled")),
    );

    assert_all(&misses);
    disabled.assert();
    post.assert();
}

#[test]
fn resolve_parent_not_found_after_a_5xx_is_not_reported_as_missing() {
    let mut env = Env::new();
    let misses = env.misses(&[PAGES, AREA, AGENT, REPORT], "/sell/");
    let unavailable = env.fails(VALUATION, "/sell/", 503, "unavailable");

    // The parent may be on the page type that could not be searched, so
    // this is a retryable failure, not exit 4 "no such parent".
    env.create_page("/sell/", &["--max-retries", "0"])
        .code(predicate::ne(4))
        .stderr(predicate::str::contains("503"));

    assert_all(&misses);
    unavailable.assert();
}

// market-reports create needs a real search.

#[test]
fn market_reports_create_rejects_a_null_or_empty_search() {
    let mut env = Env::new();
    let post = env.never("POST", REPORT);
    let given: [&[&str]; 5] = [
        &["--search", "null"],
        &["--data", r#"{"search": null}"#],
        &["--data", r#"{"search": {}}"#],
        &["--data", r#"{"search": ""}"#],
        &["--data", r#"{"search": []}"#],
    ];
    for args in given {
        env.cmd()
            .args([
                "market-reports",
                "create",
                "--slug",
                "q1",
                "--anchor-text",
                "Q1 Report",
            ])
            .args(args)
            .assert()
            .code(2)
            .stderr(predicate::str::contains(
                "--search-criteria or --search is required to create a market report",
            ));
    }
    post.assert();
}
