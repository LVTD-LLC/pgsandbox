use uuid::Uuid;

const MAX_IDENTIFIER_LENGTH: usize = 63;

pub struct SandboxNames {
    pub database_id: String,
    pub database_name: String,
    pub role_name: String,
}

pub fn slugify_name_hint(value: Option<&str>) -> String {
    let mut slug = String::new();
    let mut last_was_separator = false;

    for character in value.unwrap_or("sandbox").chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator && !slug.is_empty() {
            slug.push('_');
            last_was_separator = true;
        }
    }

    let slug = slug.trim_matches('_').to_string();
    if slug.is_empty() {
        "sandbox".to_string()
    } else {
        slug
    }
}

pub fn make_sandbox_names(prefix: &str, name_hint: Option<&str>) -> SandboxNames {
    let database_id = Uuid::new_v4().to_string();
    let short_id = database_id
        .replace('-', "")
        .chars()
        .take(10)
        .collect::<String>();
    let prefix = slugify_name_hint(Some(prefix));
    let hint = slugify_name_hint(name_hint)
        .chars()
        .take(28)
        .collect::<String>();
    let base = trim_identifier(&format!("{prefix}_{hint}_{short_id}"));
    let role_base = trim_identifier(base.trim_end_matches('_'));
    let role_prefix = role_base
        .chars()
        .take(MAX_IDENTIFIER_LENGTH - 5)
        .collect::<String>()
        .trim_end_matches('_')
        .to_string();

    SandboxNames {
        database_id,
        database_name: base,
        role_name: format!("{role_prefix}_role"),
    }
}

pub fn quote_ident(identifier: &str) -> anyhow::Result<String> {
    if identifier.is_empty() || identifier.len() > MAX_IDENTIFIER_LENGTH {
        anyhow::bail!("Invalid Postgres identifier length: {identifier}");
    }
    Ok(format!("\"{}\"", identifier.replace('"', "\"\"")))
}

pub fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn trim_identifier(value: &str) -> String {
    value
        .chars()
        .take(MAX_IDENTIFIER_LENGTH)
        .collect::<String>()
        .trim_end_matches('_')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugifies_name_hints() {
        assert_eq!(slugify_name_hint(Some("Bug #123 repro")), "bug_123_repro");
        assert_eq!(slugify_name_hint(Some("!!!")), "sandbox");
    }

    #[test]
    fn generated_names_fit_postgres_identifier_limit() {
        let names = make_sandbox_names(
            "pgsandbox",
            Some("a very long migration validation task name"),
        );

        assert!(names.database_name.len() <= 63);
        assert!(names.role_name.len() <= 63);
        assert!(names.role_name.ends_with("_role"));
    }

    #[test]
    fn quotes_identifiers_and_literals() {
        assert_eq!(quote_ident("safe_name").unwrap(), "\"safe_name\"");
        assert_eq!(quote_ident("has\"quote").unwrap(), "\"has\"\"quote\"");
        assert_eq!(quote_literal("can't"), "'can''t'");
    }
}
