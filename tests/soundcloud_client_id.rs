mod support;

use maxfetch::config::Credentials;
use maxfetch::sources::soundcloud::{
    SoundCloudEndpoints, cached_client_id, ensure_client_id, find_client_id, get_user,
    scrape_client_id, script_urls, store_client_id,
};
use support::{MockServer, Reply, Route, fixture};

fn read(rel: &str) -> String {
    std::fs::read_to_string(fixture(rel)).unwrap()
}

fn endpoints(base: &str) -> SoundCloudEndpoints {
    SoundCloudEndpoints {
        web_base: base.into(),
        api_base: base.into(),
        script_base: base.into(),
    }
}

fn server(late_hits: &str) -> impl std::future::Future<Output = MockServer> {
    let home = read("soundcloud/home.html");
    let late = late_hits.to_string();
    async move {
        MockServer::start(vec![
            Route::new("/", vec![Reply::ok("text/html", home)]),
            Route::new(
                "/assets/0-early.js",
                vec![Reply::ok(
                    "application/javascript",
                    read("soundcloud/bundle_early.js"),
                )],
            ),
            Route::new(
                "/assets/50-late.js",
                vec![Reply::ok("application/javascript", late)],
            ),
        ])
        .await
    }
}

#[test]
fn every_script_src_is_collected_in_document_order() {
    let urls = script_urls(&read("soundcloud/home.html"));
    assert_eq!(urls.len(), 2);
    assert_eq!(urls[0], "https://a-v2.sndcdn.com/assets/0-early.js");
    assert_eq!(urls[1], "https://a-v2.sndcdn.com/assets/50-late.js");
}

#[test]
fn a_client_id_is_found_in_the_recorded_bundle() {
    let js = read("soundcloud/bundle_late.js");
    assert_eq!(
        find_client_id(&js).as_deref(),
        Some("iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz")
    );
}

#[test]
fn a_bundle_without_a_client_id_is_skipped() {
    assert_eq!(find_client_id(&read("soundcloud/bundle_early.js")), None);
}

#[test]
fn an_id_that_is_not_thirty_two_characters_is_rejected() {
    assert_eq!(
        find_client_id(r#"{"client_id":"iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjm"}"#),
        None
    );
    assert_eq!(
        find_client_id(r#"{"client_id":"iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmzz"}"#),
        None
    );
    assert_eq!(
        find_client_id(r#"{"api_key":"iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz"}"#),
        None
    );
}

#[tokio::test]
async fn scripts_are_walked_in_reverse_order_of_appearance() {
    let late = read("soundcloud/bundle_late.js");
    let server = server(&late).await;
    let client = reqwest::Client::new();
    let id = scrape_client_id(&client, &endpoints(&server.base_url()))
        .await
        .unwrap();
    assert_eq!(id, "iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz");
    assert_eq!(server.request_count("/assets/50-late.js"), 1);
    assert_eq!(server.request_count("/assets/0-early.js"), 0);
}

#[tokio::test]
async fn a_page_without_a_usable_bundle_is_an_error() {
    let server = server(&read("soundcloud/bundle_early.js")).await;
    let client = reqwest::Client::new();
    let err = scrape_client_id(&client, &endpoints(&server.base_url()))
        .await
        .unwrap_err();
    assert!(format!("{err}").contains("client_id not found"));
}

#[tokio::test]
async fn the_scraped_id_is_saved_and_read_back_from_credentials() {
    let server = server(&read("soundcloud/bundle_late.js")).await;
    let dir = tempfile::tempdir().unwrap();
    let client = reqwest::Client::new();
    let id = ensure_client_id(dir.path(), &client, &endpoints(&server.base_url()))
        .await
        .unwrap();
    assert_eq!(id, "iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz");
    assert_eq!(
        cached_client_id(dir.path()).as_deref(),
        Some("iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz")
    );
    let creds = Credentials::load_from(dir.path()).unwrap();
    assert_eq!(
        creds.soundcloud.unwrap().client_id,
        "iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz"
    );
}

#[tokio::test]
async fn a_cached_id_is_reused_without_touching_the_network() {
    let server = server(&read("soundcloud/bundle_late.js")).await;
    let dir = tempfile::tempdir().unwrap();
    store_client_id(dir.path(), "iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz").unwrap();
    let client = reqwest::Client::new();
    let id = ensure_client_id(dir.path(), &client, &endpoints(&server.base_url()))
        .await
        .unwrap();
    assert_eq!(id, "iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz");
    assert_eq!(server.request_count("/"), 0);
}

#[tokio::test]
async fn the_artist_profile_is_read_from_the_users_endpoint() {
    let server = MockServer::start(vec![Route::new(
        "/users/77",
        vec![Reply::ok("application/json", read("soundcloud/user.json"))],
    )])
    .await;
    let client = reqwest::Client::new();
    let user = get_user(
        &client,
        &endpoints(&server.base_url()),
        "iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz",
        77,
    )
    .await
    .unwrap();
    assert_eq!(user.id, 77);
    assert_eq!(user.username, "nightwave");
    assert_eq!(user.permalink, "nightwave");
    assert_eq!(user.followers_count, 18244);
    assert_eq!(user.city.as_deref(), Some("Berlin"));
    assert_eq!(user.country.as_deref(), Some("DE"));
    assert_eq!(server.request_count("/users/77"), 1);
}
