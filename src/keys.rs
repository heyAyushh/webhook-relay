pub fn github_dedup_key(delivery_id: &str, action: &str, entity_id: &str) -> String {
    format!("github:{delivery_id}:{action}:{entity_id}")
}

pub fn linear_dedup_key(delivery_id: &str, action: &str, entity_id: &str) -> String {
    format!("linear:{delivery_id}:{action}:{entity_id}")
}

pub fn github_cooldown_key(repo: &str, entity_id: &str) -> String {
    let repo_token = repo.replace('/', "-");
    format!("cooldown-github-{repo_token}-{entity_id}")
}

pub fn linear_cooldown_key(team_key: &str, entity_id: &str) -> String {
    format!("cooldown-linear-{team_key}-{entity_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_dedup_key_matches_current_script_shape() {
        assert_eq!(
            github_dedup_key("delivery-1", "opened", "42"),
            "github:delivery-1:opened:42"
        );
    }

    #[test]
    fn linear_dedup_key_matches_current_script_shape() {
        assert_eq!(
            linear_dedup_key("delivery-2", "create", "issue-42"),
            "linear:delivery-2:create:issue-42"
        );
    }

    #[test]
    fn github_cooldown_key_matches_current_script_shape() {
        assert_eq!(
            github_cooldown_key("org/repo", "42"),
            "cooldown-github-org-repo-42"
        );
    }

    #[test]
    fn linear_cooldown_key_matches_current_script_shape() {
        assert_eq!(
            linear_cooldown_key("ENG", "issue-42"),
            "cooldown-linear-ENG-issue-42"
        );
    }
}
