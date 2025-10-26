use futures::stream::FuturesUnordered;
use futures::StreamExt;
use once_cell::sync::Lazy;
use reqwest::Client;
use std::{
    fs::File,
    io::Write,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};
use tokio::signal;
use tokio::task;

const USER_AGENT: &str = "Mozilla/5.0";
const UID_START: u32 = 10_000;
const UID_END: u32 = 80_000;
const QUESTION_COUNT: u32 = 300;
const MAX_CONCURRENCY: usize = 20;

static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
static VALID_LOGGER: Lazy<Mutex<File>> = Lazy::new(|| {
    Mutex::new(File::create("valid_urls.log").expect("Unable to create log file"))
});

/// Graceful shutdown (Ctrl+C)
async fn handle_signals() {
    signal::ctrl_c()
        .await
        .expect("Failed to install Ctrl+C handler");
    println!("Gracefully stopping... (Ctrl+C again to force quit)");
    STOP_REQUESTED.store(true, Ordering::SeqCst);
}

/// Check if a URL is valid (status 200, no redirects)
async fn check_url(client: &Client, base_template: &str, uid: u32, qnum: u32) -> Option<String> {
    if STOP_REQUESTED.load(Ordering::SeqCst) {
        return None;
    }

    let url = base_template
        .replace("{uid}", &uid.to_string())
        .replace("{qnum}", &qnum.to_string());

    println!("Trying: {}", url);

    match client
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            if status == 200 && resp.url().as_str() == url {
                println!("Found: {}", url);
                let mut log = VALID_LOGGER.lock().unwrap();
                writeln!(log, "{}", url).ok();
                return Some(url);
            } else if status.as_u16() >= 400 {
                println!("[BAD] {} - {}", status, url);
            }
        }
        Err(err) => {
            println!("[ERROR] {} - {}", url, err);
        }
    }

    None
}

/// Search for a valid URL for one question
async fn find_valid_url_for_question(client: &Client, base_template: &str, qnum: u32) {
    let mut futures: FuturesUnordered<_> = FuturesUnordered::new();

    for uid in UID_START..UID_END {
        if STOP_REQUESTED.load(Ordering::SeqCst) {
            break;
        }

        while futures.len() >= MAX_CONCURRENCY {
            if let Some(res) = futures.next().await {
                if let Some(_url) = res {
                    return; // Found a valid one
                }
            }
        }

        futures.push(check_url(client, base_template, uid, qnum));
    }

    while let Some(res) = futures.next().await {
        if STOP_REQUESTED.load(Ordering::SeqCst) {
            break;
        }
        if let Some(_url) = res {
            return;
        }
    }
}

/// ## Usage
///
/// cargo run --release \
///   "https://www.examtopics.com/discussions/splunk/view/{uid}-exam-splk-1003-topic-1-question-{qnum}-discussion/" \
///   1
/// ```
///
/// - The first argument is the **base URL template** containing `{uid}` and `{qnum}` placeholders.
/// - The second argument (optional) is the **starting question number** (default: 1).
#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: {} <BASE_URL_TEMPLATE> [START_QNUM]", args[0]);
        eprintln!("Example:");
        eprintln!(
            "  {} \"https://www.examtopics.com/discussions/splunk/view/{{uid}}-exam-splk-1003-topic-1-question-{{qnum}}-discussion/\" 1",
            args[0]
        );
        std::process::exit(1);
    }

    let base_template = &args[1];
    let start_qnum = args
        .get(2)
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(1);

    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("Failed to create client");

    // Spawn signal handler
    task::spawn(handle_signals());

    for qnum in start_qnum..=QUESTION_COUNT {
        if STOP_REQUESTED.load(Ordering::SeqCst) {
            break;
        }
        println!("Searching for Question {}...", qnum);
        find_valid_url_for_question(&client, base_template, qnum).await;
    }

    println!("Exiting.");
}
