#![expect(clippy::print_stdout, reason = "a command-line check")]

use dumbj_review::{Client, PrFilter, RepoId, ReviewError, token};

fn main() -> Result<(), ReviewError> {
    let url = std::env::args().nth(1).unwrap_or_default();
    let Some(repo) = RepoId::from_remote_url(&url) else {
        println!("usage: pulls <remote-url>");
        return Ok(());
    };
    let client = Client::new(&repo.host, token(&repo.host)?);
    let pulls = client.pull_requests(&repo, PrFilter::All, "")?;
    println!(
        "{} open pull requests in {}/{}",
        pulls.len(),
        repo.owner,
        repo.name
    );
    for pull in pulls {
        println!("#{} {} ({})", pull.number, pull.title, pull.author);
    }
    Ok(())
}
