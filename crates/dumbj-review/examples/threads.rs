//! Prints a pull request's details and review threads with the `gh` token, as a read-only check
//! against the real API: `cargo run -p dumbj-review --example threads -- <remote-url> <number>`.

#![expect(clippy::print_stdout, reason = "a command-line check")]

use dumbj_review::{Client, RepoId, ReviewError, token};

fn main() -> Result<(), ReviewError> {
    let mut args = std::env::args().skip(1);
    let url = args.next().unwrap_or_default();
    let number = args.next().and_then(|n| n.parse::<u64>().ok());
    let (Some(repo), Some(number)) = (RepoId::from_remote_url(&url), number) else {
        println!("usage: threads <remote-url> <number>");
        return Ok(());
    };
    let client = Client::new(&repo.host, token(&repo.host)?);
    let pr = client.pull_request(&repo, number)?;
    println!(
        "#{} {} ({}, {:?})",
        pr.number, pr.title, pr.author, pr.state
    );
    println!("head {} ({})", pr.head_oid, pr.head_ref);
    println!("base {} ({})", pr.base_oid, pr.base_ref);
    println!("pending review: {:?}", pr.pending_review);
    println!("checks: {}", pr.checks.len());
    for check in &pr.checks {
        println!("  {:?} {}", check.state, check.name);
    }
    println!("commits: {}", pr.commits.len());
    for commit in &pr.commits {
        println!(
            "  {} {} ({}, {})",
            &commit.oid[..commit.oid.len().min(8)],
            commit.headline,
            commit.author,
            commit.date
        );
    }
    let threads = client.review_threads(&repo, number)?;
    println!("threads: {}", threads.len());
    for thread in threads {
        println!(
            "  {} {:?} {:?}-{:?} resolved={} outdated={}",
            thread.path,
            thread.side,
            thread.start_line,
            thread.line,
            thread.is_resolved,
            thread.is_outdated
        );
        for comment in thread.comments {
            println!("    {}: {}", comment.author, comment.body);
        }
    }
    Ok(())
}
