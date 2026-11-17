use serde::Deserialize;

use crate::ReviewError;

#[derive(Deserialize)]
pub(crate) struct Author {
    pub(crate) login: String,
}

pub(crate) fn login(author: Option<Author>) -> String {
    author.map_or_else(|| "ghost".to_string(), |a| a.login)
}

#[derive(Deserialize)]
pub(crate) struct RepoData<T> {
    repository: Option<RepoNode<T>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoNode<T> {
    pull_request: Option<T>,
}

impl<T> RepoData<T> {
    pub(crate) fn pull_request(self, number: u64) -> Result<T, ReviewError> {
        self.repository
            .and_then(|r| r.pull_request)
            .ok_or_else(|| ReviewError::Decode(format!("pull request #{number} not found")))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PageInfo {
    pub(crate) has_next_page: bool,
    pub(crate) end_cursor: Option<String>,
}

impl PageInfo {
    pub(crate) fn next(&self) -> Result<Option<String>, ReviewError> {
        if !self.has_next_page {
            return Ok(None);
        }
        self.end_cursor
            .clone()
            .map(Some)
            .ok_or_else(|| ReviewError::Decode("page has more results but no cursor".to_string()))
    }
}

pub(crate) fn missing(what: &str) -> ReviewError {
    ReviewError::Decode(format!("GitHub returned no {what}"))
}
