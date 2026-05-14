use crate::{GitInfo, PreviewData, PreviewExtraData};

/// Convert a git Preview worker result into the next Preview state.
pub fn apply_git_preview_result(result: Result<GitInfo, String>) -> PreviewData {
    match result {
        Ok(info) => PreviewData::Ready(info),
        Err(error) => PreviewData::Error(error),
    }
}

/// Merge extra Preview data into an already-ready Preview state.
pub fn apply_extra_preview_result(preview_data: &mut PreviewData, extra: PreviewExtraData) -> bool {
    if let PreviewData::Ready(info) = preview_data {
        info.pr = extra.pr;
        info.ci = Some(extra.ci);
        info.zellij = extra.zellij;
        info.review = extra.review;
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CommitInfo, ZellijInfo};
    use zigzag_core::domain::{CiStatus, PrState, PullRequest, ReviewStatus};

    fn git_info() -> GitInfo {
        GitInfo {
            branch: "main".to_string(),
            ahead: 0,
            behind: 0,
            is_dirty: false,
            commits: vec![CommitInfo {
                hash: "abc123".to_string(),
                message: "init".to_string(),
            }],
            pr: None,
            ci: None,
            zellij: None,
            review: None,
        }
    }

    #[test]
    fn git_preview_success_becomes_ready() {
        let preview = apply_git_preview_result(Ok(git_info()));

        assert!(matches!(preview, PreviewData::Ready(_)));
    }

    #[test]
    fn git_preview_error_becomes_error() {
        let preview = apply_git_preview_result(Err("not a repo".to_string()));

        assert!(matches!(preview, PreviewData::Error(error) if error == "not a repo"));
    }

    #[test]
    fn extra_preview_merges_into_ready_state() {
        let mut preview = PreviewData::Ready(git_info());
        let extra = PreviewExtraData {
            pr: Some(PullRequest {
                number: 42,
                title: "Fix".to_string(),
                url: "https://example.com/pr/42".to_string(),
                state: PrState::Open,
            }),
            ci: CiStatus::Passing,
            zellij: Some(ZellijInfo {
                tab_count: 2,
                pane_count: 3,
                uptime: "1h".to_string(),
            }),
            review: Some(ReviewStatus {
                has_new_comments: true,
                comment_count: 1,
                last_review_at: Some("2026-05-10T09:00:00Z".to_string()),
            }),
        };

        assert!(apply_extra_preview_result(&mut preview, extra));
        match preview {
            PreviewData::Ready(info) => {
                assert_eq!(info.pr.as_ref().map(|pr| pr.number), Some(42));
                assert_eq!(info.ci, Some(CiStatus::Passing));
                assert_eq!(info.zellij.as_ref().map(|zellij| zellij.tab_count), Some(2));
                assert_eq!(
                    info.review.as_ref().map(|review| review.comment_count),
                    Some(1)
                );
            }
            _ => panic!("expected ready preview"),
        }
    }

    #[test]
    fn extra_preview_does_not_mutate_non_ready_state() {
        let mut preview = PreviewData::Loading;
        let extra = PreviewExtraData {
            pr: None,
            ci: CiStatus::Unknown,
            zellij: None,
            review: None,
        };

        assert!(!apply_extra_preview_result(&mut preview, extra));
        assert!(matches!(preview, PreviewData::Loading));
    }
}
