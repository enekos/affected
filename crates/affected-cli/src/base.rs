/// The ref `--base auto` stands for: `AFFECTED_BASE`, else the target branch of the pull or
/// merge request CI is building, else `origin/HEAD`. Empty variables count as unset.
///
/// Mirrors `detectBase` in the Node package.
pub fn detect_base(env: impl Fn(&str) -> Option<String>) -> String {
    let set = |name: &str| env(name).filter(|value| !value.is_empty());
    if let Some(base) = set("AFFECTED_BASE") {
        return base;
    }
    let target = [
        "GITHUB_BASE_REF",
        "CI_MERGE_REQUEST_TARGET_BRANCH_NAME",
        "BUILDKITE_PULL_REQUEST_BASE_BRANCH",
        "BITBUCKET_PR_DESTINATION_BRANCH",
    ]
    .into_iter()
    .find_map(set)
    .or_else(|| {
        // Azure Pipelines gives a full ref.
        set("SYSTEM_PULLREQUEST_TARGETBRANCH")
            .map(|branch| branch.strip_prefix("refs/heads/").map_or_else(|| branch.clone(), str::to_owned))
            .filter(|branch| !branch.is_empty())
    });
    target.map_or_else(|| "origin/HEAD".to_owned(), |target| format!("origin/{target}"))
}

#[cfg(test)]
mod tests {
    use super::detect_base;

    fn detect(vars: &[(&str, &str)]) -> String {
        detect_base(|name| vars.iter().find(|(key, _)| *key == name).map(|(_, value)| (*value).to_owned()))
    }

    #[test]
    fn affected_base_wins_over_ci_variables() {
        assert_eq!(detect(&[("AFFECTED_BASE", "upstream/dev"), ("GITHUB_BASE_REF", "main")]), "upstream/dev");
    }

    #[test]
    fn ci_target_branches_are_remote_refs() {
        assert_eq!(detect(&[("GITHUB_BASE_REF", "main")]), "origin/main");
        assert_eq!(detect(&[("CI_MERGE_REQUEST_TARGET_BRANCH_NAME", "develop")]), "origin/develop");
        assert_eq!(detect(&[("BUILDKITE_PULL_REQUEST_BASE_BRANCH", "trunk")]), "origin/trunk");
        assert_eq!(detect(&[("BITBUCKET_PR_DESTINATION_BRANCH", "master")]), "origin/master");
        assert_eq!(detect(&[("SYSTEM_PULLREQUEST_TARGETBRANCH", "refs/heads/release/2")]), "origin/release/2");
    }

    #[test]
    fn empty_variables_fall_through_to_origin_head() {
        assert_eq!(detect(&[]), "origin/HEAD");
        assert_eq!(detect(&[("AFFECTED_BASE", ""), ("GITHUB_BASE_REF", "")]), "origin/HEAD");
        assert_eq!(detect(&[("SYSTEM_PULLREQUEST_TARGETBRANCH", "refs/heads/")]), "origin/HEAD");
    }
}
