//! Minimal template engine: `{{key}}`, nothing else.
//!
//! Deliberately without conditionals or loops — a hook that needs logic is a shell
//! script, and the shell already does that very well.

use std::collections::BTreeMap;

pub type Vars = BTreeMap<String, String>;

/// Replaces known `{{key}}`s. Unknown keys are left as-is: a command failing while
/// showing `{{port.vite}}` beats one silently missing an argument.
pub fn render(input: &str, vars: &Vars) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end) = input[i + 2..].find("}}") {
                let key = input[i + 2..i + 2 + end].trim();
                if let Some(value) = vars.get(key) {
                    out.push_str(value);
                    i += 2 + end + 2;
                    continue;
                }
            }
        }
        // Advance by a whole character: `input[i..]` may start with a multi-byte one.
        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Resolves the `[vars]` of wt.toml, which may reference one another. Five passes are
/// enough for any reasonable chain and cut cycles short.
pub fn expand(base: &Vars, user: &BTreeMap<String, String>) -> Vars {
    let mut vars = base.clone();
    for (k, v) in user {
        vars.insert(k.clone(), v.clone());
    }
    for _ in 0..5 {
        let mut changed = false;
        let snapshot = vars.clone();
        for key in user.keys() {
            let current = snapshot.get(key).cloned().unwrap_or_default();
            let rendered = render(&current, &snapshot);
            if rendered != current {
                vars.insert(key.clone(), rendered);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    vars
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Vars {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn replaces_known_keys() {
        let v = vars(&[("slug", "demo"), ("port.vite", "5200")]);
        assert_eq!(
            render("http://{{slug}}:{{ port.vite }}", &v),
            "http://demo:5200"
        );
    }

    #[test]
    fn leaves_unknown_keys_alone() {
        let v = vars(&[]);
        assert_eq!(render("a {{nope}} b", &v), "a {{nope}} b");
    }

    #[test]
    fn preserves_shell_braces() {
        let v = vars(&[]);
        assert_eq!(render("awk '{print $1}'", &v), "awk '{print $1}'");
    }

    #[test]
    fn resolves_chained_vars() {
        let base = vars(&[("slug", "demo")]);
        let user: BTreeMap<String, String> = vars(&[
            ("host", "{{slug}}.wt.localhost"),
            ("url", "http://{{host}}"),
        ]);
        let out = expand(&base, &user);
        assert_eq!(out["url"], "http://demo.wt.localhost");
    }

    #[test]
    fn does_not_loop_on_a_cycle() {
        let user: BTreeMap<String, String> = vars(&[("a", "{{b}}"), ("b", "{{a}}")]);
        let out = expand(&Vars::new(), &user);
        assert!(out.contains_key("a"));
    }
}
