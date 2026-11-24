use std::ffi::OsString;

pub const MAX_REQUIREMENT_LENGTH: usize = 2_048;
pub const CODESIGN_PATH: &str = "/usr/bin/codesign";
pub const DITTO_PATH: &str = "/usr/bin/ditto";
const DESIGNATED_PREFIX: &str = "designated => ";
/// `codesign -d -r-` prints an existence test on a certificate field as
/// `certificate 1[field.…] /* exists */`, a comment in the requirement
/// language. It is the one comment ever accepted, and only right after a
/// bracketed field outside a quoted string; it becomes the equivalent keyword.
const EXISTS_ANNOTATION: &str = "] /* exists */";
const EXISTS_KEYWORD: &str = "] exists";
const TEAM_PIN: &str = "certificate leaf[subject.OU] = ";

/// The requirement as checked and as handed to `codesign -R`: codesign's own
/// ` /* exists */` annotation rewritten to `exists`, which compiles to the same
/// requirement. Every other byte, and any other comment, is left as it is, so
/// [`is_enforceable_requirement`] still refuses it.
#[must_use]
pub fn normalized_requirement(requirement: &str) -> String {
    let mut normalized = String::with_capacity(requirement.len());
    let mut rest = requirement;
    let mut quoted = false;
    while let Some(character) = rest.chars().next() {
        if !quoted && rest.starts_with(EXISTS_ANNOTATION) {
            normalized.push_str(EXISTS_KEYWORD);
            rest = &rest[EXISTS_ANNOTATION.len()..];
            continue;
        }
        let mut length = character.len_utf8();
        if quoted && character == '\\' {
            length += rest[length..].chars().next().map_or(0, char::len_utf8);
        } else if character == '"' {
            quoted = !quoted;
        }
        normalized.push_str(&rest[..length]);
        rest = &rest[length..];
    }
    normalized
}

#[must_use]
pub fn is_enforceable_requirement(requirement: &str) -> bool {
    requirement.len() <= MAX_REQUIREMENT_LENGTH
        && is_enforceable_normalized(&normalized_requirement(requirement))
}

fn is_enforceable_normalized(requirement: &str) -> bool {
    let lower = requirement.to_ascii_lowercase();
    !requirement.is_empty()
        && requirement.len() <= MAX_REQUIREMENT_LENGTH
        && !requirement.chars().any(char::is_control)
        && !lower.contains(" or ")
        && !requirement.contains("||")
        && !requirement.contains("/*")
        && !requirement.contains("*/")
        && requirement.starts_with("identifier \"")
        && requirement.contains("anchor apple generic")
        && (requirement.contains("certificate leaf[subject.CN] = \"") || pins_team(requirement))
}

/// A leaf pinned by its team (subject OU). codesign quotes the value only
/// when it starts with a digit, so both `= "1A2B3C4D5E"` and `= EQHXZ8M8AV`
/// are what it prints for a real team.
fn pins_team(requirement: &str) -> bool {
    requirement
        .split_once(TEAM_PIN)
        .and_then(|(_, value)| value.chars().next())
        .is_some_and(|first| first == '"' || first.is_ascii_alphanumeric())
}

/// The one designated requirement codesign printed, normalized (see
/// [`normalized_requirement`]), or `None` when it is missing, repeated or not
/// enforceable.
#[must_use]
pub fn parse_designated_requirement(codesign_output: &str) -> Option<String> {
    let mut matching = codesign_output
        .lines()
        .filter_map(|line| line.strip_prefix(DESIGNATED_PREFIX));
    let requirement = matching.next()?.trim();
    if matching.next().is_some() || !is_enforceable_requirement(requirement) {
        None
    } else {
        Some(normalized_requirement(requirement))
    }
}

#[must_use]
pub fn requirement_arguments(requirement: &str, target: &std::path::Path) -> Vec<OsString> {
    [
        OsString::from("--verify"),
        OsString::from("--deep"),
        OsString::from("--strict"),
        OsString::from("-R"),
        OsString::from(format!("={}", normalized_requirement(requirement))),
        target.as_os_str().to_owned(),
    ]
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUIREMENT: &str = "identifier \"com.andreromero.interface\" and anchor apple generic and certificate leaf[subject.OU] = \"TEAM123\"";

    /// `codesign -d -r-` on an app signed with a free Apple Development
    /// certificate and no explicit requirement (every build up to 0.3.181;
    /// the name and person id are made up, the shape is codesign's).
    const AUTOMATIC_APPLE_DEVELOPMENT: &str = "Executable=/Applications/Interface.app/Contents/MacOS/Interface\ndesignated => identifier \"com.andreromero.interface\" and anchor apple generic and certificate leaf[subject.CN] = \"Apple Development: builder@example.com (ABCDE12345)\" and certificate 1[field.1.2.840.113635.100.6.2.1] /* exists */\n";
    /// The automatic requirement codesign derives for a Developer ID
    /// Application identity: an unquoted team, two existence annotations.
    const AUTOMATIC_DEVELOPER_ID: &str = "Executable=/Applications/Interface.app/Contents/MacOS/Interface\ndesignated => identifier \"com.andreromero.interface\" and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] /* exists */ and certificate leaf[field.1.2.840.113635.100.6.1.13] /* exists */ and certificate leaf[subject.OU] = TEAMID1234\n";
    /// Google Chrome's, as codesign prints it: Developer ID with alternatives.
    const CHROME: &str = "designated => (identifier \"com.google.Chrome\" or identifier \"com.google.Chrome.beta\" or identifier \"com.google.Chrome.dev\" or identifier \"com.google.Chrome.canary\") and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] /* exists */ and certificate leaf[field.1.2.840.113635.100.6.1.13] /* exists */ and certificate leaf[subject.OU] = EQHXZ8M8AV\n";
    /// What codesign prints back for the explicit requirement
    /// `scripts/sign-mac.cjs` signs with (a team id starting with a digit is
    /// printed quoted).
    const EXPLICIT_TEAM_PINNED: &str = "Executable=/private/tmp/Interface.app/Contents/MacOS/Interface\ndesignated => identifier \"com.andreromero.interface\" and anchor apple generic and certificate leaf[subject.OU] = \"1A2B3C4D5E\"\n";

    #[test]
    fn requirement_must_pin_identifier_apple_chain_and_leaf() {
        assert!(is_enforceable_requirement(REQUIREMENT));
        assert!(!is_enforceable_requirement("anchor apple"));
        assert!(!is_enforceable_requirement(
            "identifier \"com.andreromero.interface\" and anchor apple generic"
        ));
        assert!(!is_enforceable_requirement(&format!(
            "{REQUIREMENT}\nforged"
        )));
        assert!(!is_enforceable_requirement(&format!(
            "{REQUIREMENT} or true"
        )));
        assert!(!is_enforceable_requirement(&format!(
            "{REQUIREMENT} /* comment */"
        )));
    }

    #[test]
    fn designated_parser_refuses_comments_and_ambiguous_claims() {
        assert_eq!(
            parse_designated_requirement(&format!("designated => {REQUIREMENT}\n")),
            Some(REQUIREMENT.into())
        );
        assert_eq!(
            parse_designated_requirement(&format!(
                "designated => {REQUIREMENT}\ndesignated => {REQUIREMENT}\n"
            )),
            None
        );
        assert_eq!(
            parse_designated_requirement(&format!("# designated => {REQUIREMENT}\n")),
            None
        );
    }

    #[test]
    fn codesign_requirements_of_real_builds_are_read_with_exists_spelled_out() {
        assert_eq!(
            parse_designated_requirement(AUTOMATIC_APPLE_DEVELOPMENT).as_deref(),
            Some(
                "identifier \"com.andreromero.interface\" and anchor apple generic and certificate leaf[subject.CN] = \"Apple Development: builder@example.com (ABCDE12345)\" and certificate 1[field.1.2.840.113635.100.6.2.1] exists"
            )
        );
        assert_eq!(
            parse_designated_requirement(AUTOMATIC_DEVELOPER_ID).as_deref(),
            Some(
                "identifier \"com.andreromero.interface\" and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = TEAMID1234"
            )
        );
        assert_eq!(
            parse_designated_requirement(EXPLICIT_TEAM_PINNED).as_deref(),
            Some(
                "identifier \"com.andreromero.interface\" and anchor apple generic and certificate leaf[subject.OU] = \"1A2B3C4D5E\""
            )
        );
        // Alternatives stay refused even when every annotation is codesign's.
        assert_eq!(parse_designated_requirement(CHROME), None);
    }

    #[test]
    fn only_codesigns_exists_annotation_after_a_field_is_accepted() {
        let field = "certificate 1[field.1.2.840.113635.100.6.2.1]";
        assert!(is_enforceable_requirement(&format!(
            "{REQUIREMENT} and {field} /* exists */"
        )));
        for refused in [
            format!("{REQUIREMENT} and {field} /* exists*/"),
            format!("{REQUIREMENT} and {field} /*exists*/"),
            format!("{REQUIREMENT} and {field} /* exists */ */"),
            format!("{REQUIREMENT} and {field} /* /* exists */"),
            format!("{REQUIREMENT} and {field} /* absent */"),
            format!("{REQUIREMENT} /* exists */"),
            format!("{REQUIREMENT} and anchor apple /* exists */"),
            format!("{REQUIREMENT} and {field} /* exists */ or anchor apple"),
            // Inside a quoted string the text is a value, not an annotation.
            "identifier \"com.andreromero.interface] /* exists */\" and anchor apple generic and certificate leaf[subject.OU] = \"TEAM123\"".to_owned(),
            // An existence test on the team is not a pin on it.
            "identifier \"com.andreromero.interface\" and anchor apple generic and certificate leaf[subject.OU] /* exists */".to_owned(),
        ] {
            assert!(!is_enforceable_requirement(&refused), "{refused}");
            assert_eq!(
                parse_designated_requirement(&format!("designated => {refused}\n")),
                None,
                "{refused}"
            );
        }
    }

    #[test]
    fn codesign_is_handed_the_normalized_requirement() {
        let annotated =
            format!("{REQUIREMENT} and certificate 1[field.1.2.840.113635.100.6.2.1] /* exists */");
        let arguments = requirement_arguments(&annotated, std::path::Path::new("/x.app"));
        assert_eq!(
            arguments[4],
            OsString::from(format!(
                "={REQUIREMENT} and certificate 1[field.1.2.840.113635.100.6.2.1] exists"
            ))
        );
        let normalized = normalized_requirement(&annotated);
        assert_eq!(normalized_requirement(&normalized), normalized);
        assert_eq!(normalized_requirement(REQUIREMENT), REQUIREMENT);
    }
}
