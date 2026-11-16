use serde::Deserialize;
use serde_json::json;

use crate::wire::missing;
use crate::{Client, ReviewError};

const ADD_REVIEW_MUTATION: &str = "mutation($input: AddPullRequestReviewInput!) {
  addPullRequestReview(input: $input) { pullRequestReview { id } }
}";

const SUBMIT_MUTATION: &str = "mutation($input: SubmitPullRequestReviewInput!) {
  submitPullRequestReview(input: $input) { pullRequestReview { id } }
}";

const DELETE_MUTATION: &str = "mutation($input: DeletePullRequestReviewInput!) {
  deletePullRequestReview(input: $input) { pullRequestReview { id } }
}";

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewEvent {
    Comment,
    Approve,
    RequestChanges,
}

impl ReviewEvent {
    fn as_graphql(self) -> &'static str {
        match self {
            Self::Comment => "COMMENT",
            Self::Approve => "APPROVE",
            Self::RequestChanges => "REQUEST_CHANGES",
        }
    }
}

#[derive(Deserialize)]
struct IdOnly {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewPayload {
    pull_request_review: Option<IdOnly>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddReviewData {
    add_pull_request_review: Option<ReviewPayload>,
}

#[derive(Deserialize)]
struct Ignored {}

impl Client {
    /// Creates a pending review (no event) and returns its id.
    pub fn start_review(&self, pull_id: &str) -> Result<String, ReviewError> {
        let data: AddReviewData = self.graphql(
            ADD_REVIEW_MUTATION,
            json!({ "input": { "pullRequestId": pull_id } }),
        )?;
        data.add_pull_request_review
            .and_then(|p| p.pull_request_review)
            .map(|r| r.id)
            .ok_or_else(|| missing("review"))
    }

    pub fn submit_review(
        &self,
        review_id: &str,
        event: ReviewEvent,
        body: &str,
    ) -> Result<(), ReviewError> {
        let _: Ignored = self.graphql(
            SUBMIT_MUTATION,
            json!({ "input": {
                "pullRequestReviewId": review_id,
                "event": event.as_graphql(),
                "body": body,
            } }),
        )?;
        Ok(())
    }

    /// Posts a review with an event and no pending comments.
    pub fn add_review(
        &self,
        pull_id: &str,
        event: ReviewEvent,
        body: &str,
    ) -> Result<(), ReviewError> {
        let _: Ignored = self.graphql(
            ADD_REVIEW_MUTATION,
            json!({ "input": {
                "pullRequestId": pull_id,
                "event": event.as_graphql(),
                "body": body,
            } }),
        )?;
        Ok(())
    }

    pub fn delete_review(&self, review_id: &str) -> Result<(), ReviewError> {
        let _: Ignored = self.graphql(
            DELETE_MUTATION,
            json!({ "input": { "pullRequestReviewId": review_id } }),
        )?;
        Ok(())
    }
}
