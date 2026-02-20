pub fn is_supported_github_event_action(_event: &str, _action: &str) -> bool {
    let event_allowed = matches!(
        _event,
        "pull_request" | "pull_request_review" | "pull_request_review_comment" | "issue_comment"
    );
    if !event_allowed {
        return false;
    }

    matches!(
        _action,
        "opened" | "synchronize" | "reopened" | "submitted" | "created"
    )
}

pub fn is_supported_linear_type(_event_type: &str) -> bool {
    matches!(_event_type, "Issue" | "Comment")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_event_action_parity() {
        assert!(is_supported_github_event_action("pull_request", "opened"));
        assert!(is_supported_github_event_action(
            "pull_request_review",
            "submitted"
        ));
        assert!(is_supported_github_event_action(
            "pull_request_review_comment",
            "created"
        ));
        assert!(is_supported_github_event_action("issue_comment", "created"));

        assert!(!is_supported_github_event_action("push", "opened"));
        assert!(!is_supported_github_event_action("pull_request", "closed"));
    }

    #[test]
    fn linear_type_parity() {
        assert!(is_supported_linear_type("Issue"));
        assert!(is_supported_linear_type("Comment"));
        assert!(!is_supported_linear_type("Project"));
        assert!(!is_supported_linear_type(""));
    }
}
