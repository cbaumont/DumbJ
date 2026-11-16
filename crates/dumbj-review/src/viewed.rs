use serde::Deserialize;
use serde_json::json;

use crate::wire::{PageInfo, RepoData, missing};
use crate::{Client, RepoId, ReviewError};

const VIEWED_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!, $after: String) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      files(first: 100, after: $after) {
        pageInfo { hasNextPage endCursor }
        nodes { path viewerViewedState }
      }
    }
  }
}";

const MARK_MUTATION: &str = "mutation($input: MarkFileAsViewedInput!) {
  markFileAsViewed(input: $input) { clientMutationId }
}";

const UNMARK_MUTATION: &str = "mutation($input: UnmarkFileAsViewedInput!) {
  unmarkFileAsViewed(input: $input) { clientMutationId }
}";

#[cfg(test)]
mod tests;

#[derive(Deserialize)]
struct FilesPage {
    files: Option<Files>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Files {
    page_info: PageInfo,
    nodes: Vec<File>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct File {
    path: String,
    viewer_viewed_state: String,
}

#[derive(Deserialize)]
struct Ignored {}

impl Client {
    /// Paths the viewer marked as viewed, following pagination of the pull request's files.
    pub fn viewed_files(&self, repo: &RepoId, number: u64) -> Result<Vec<String>, ReviewError> {
        let mut viewed = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let data: RepoData<FilesPage> = self.graphql(
                VIEWED_QUERY,
                json!({ "owner": repo.owner, "name": repo.name, "number": number, "after": after }),
            )?;
            let files = data
                .pull_request(number)?
                .files
                .ok_or_else(|| missing("files"))?;
            viewed.extend(
                files
                    .nodes
                    .into_iter()
                    .filter(|f| f.viewer_viewed_state == "VIEWED")
                    .map(|f| f.path),
            );
            match files.page_info.next()? {
                Some(cursor) => after = Some(cursor),
                None => return Ok(viewed),
            }
        }
    }

    pub fn mark_file_viewed(&self, pull_id: &str, path: &str) -> Result<(), ReviewError> {
        let _: Ignored = self.graphql(
            MARK_MUTATION,
            json!({ "input": { "pullRequestId": pull_id, "path": path } }),
        )?;
        Ok(())
    }

    pub fn unmark_file_viewed(&self, pull_id: &str, path: &str) -> Result<(), ReviewError> {
        let _: Ignored = self.graphql(
            UNMARK_MUTATION,
            json!({ "input": { "pullRequestId": pull_id, "path": path } }),
        )?;
        Ok(())
    }
}
