use console::Style;
use regex::Regex;
use std::error::Error;
use std::fmt::Display;
use std::sync::LazyLock;

pub static CYAN: LazyLock<Style> = LazyLock::new(|| Style::new().cyan().bold());
pub static BLUE: LazyLock<Style> = LazyLock::new(|| Style::new().blue().bold());
pub static RED: LazyLock<Style> = LazyLock::new(|| Style::new().red().bold());
pub static DIM_RED: LazyLock<Style> = LazyLock::new(|| Style::new().red().dim());
pub static YELLOW: LazyLock<Style> = LazyLock::new(|| Style::new().yellow().bold());
pub static GREEN: LazyLock<Style> = LazyLock::new(|| Style::new().green().bold());
pub static BOLD: LazyLock<Style> = LazyLock::new(|| Style::new().bold());
pub static MAGENTA: LazyLock<Style> = LazyLock::new(|| Style::new().magenta().bold());
pub static DIM: LazyLock<Style> = LazyLock::new(|| Style::new().dim());
pub static WHITE: LazyLock<Style> = LazyLock::new(|| Style::new().white().bold());
pub static ITALIC: LazyLock<Style> = LazyLock::new(|| Style::new().italic());
pub static PLAIN: LazyLock<Style> = LazyLock::new(Style::default);

#[derive(PartialEq, Clone, Debug, Eq)]
pub enum StringFragment {
    Outer(String),
    Inner(String),
}

pub fn split_interpolation(value: &str) -> Result<Vec<StringFragment>, Box<dyn Error>> {
    if value.is_empty() {
        return Ok(vec![]);
    }

    // find '{' and '}' indices
    let pairs = value
        .chars()
        .enumerate()
        .filter(|(_, c)| *c == '{' || *c == '}')
        .map(|(i, _)| i)
        .collect::<Vec<_>>();

    // split into pieces
    if pairs.len() % 2 != 0 {
        return Err("non matching interpolation curly parenthesis".into());
    }
    let mut i = 0;
    let mut last = i;
    let value_vec: Vec<char> = value.chars().collect();
    let mut res: Vec<StringFragment> = vec![];
    while i < pairs.len() {
        let open = pairs[i];
        if value_vec[open] != '{' {
            return Err("non matching interpolation curly parenthesis".into());
        }
        if open != last {
            res.push(StringFragment::Outer(value[last..open].to_owned()));
        }
        let close = pairs[i + 1];
        if value_vec[close] != '}' {
            return Err("non matching interpolation curly parenthesis".into());
        }
        last = close + 1;
        i += 2;
        res.push(StringFragment::Inner(value[open + 1..close].to_owned()));
    }
    if pairs.is_empty() {
        res.push(StringFragment::Outer(value.to_owned()));
    } else if pairs[pairs.len() - 1] != value.len() - 1 {
        res.push(StringFragment::Outer(
            value[pairs[pairs.len() - 1] + 1..].to_owned(),
        ));
    }
    Ok(res)
}

pub fn split_quotes(value: &str) -> Vec<StringFragment> {
    split_quotes_with(value, '\'')
}

pub fn split_quotes_with(value: &str, quote: char) -> Vec<StringFragment> {
    if value.is_empty() {
        return vec![];
    }

    // First convert the String to a Vec<char>. This ensures indexes are on
    // valid UTF-8 char boundaries:
    let chars = value
        //TODO Replace DF error messages with proper form, this is a quick HACK
        .replace("doesn't", "does not")
        .chars()
        .collect::<Vec<_>>();

    // find quote indices
    let pairs = chars
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == quote)
        .map(|(i, _)| i)
        .collect::<Vec<_>>();

    if pairs.len() % 2 != 0 {
        // This gets called with various content which we can't predict-
        // for example, human-readable error messages. For now, let's play it
        // safe by returning the original string (rather than maybe losing
        // error messages).
        //
        // TODO: This approach would still apply inappropriate colors to e.g.
        // "I don't like Randy's TODO" (note that two single quotes appear)
        // We should probably use a less likely text marker, like ``, or
        // use better types.
        return vec![StringFragment::Outer(value.to_owned())];
    }
    let mut i = 0;
    let mut last = i;
    let mut res: Vec<StringFragment> = vec![];
    while i < pairs.len() {
        let open = pairs[i];
        if open != last {
            res.push(StringFragment::Outer(
                chars[last..open].iter().collect::<String>(),
            ));
        }
        let close = pairs[i + 1];
        last = close + 1;
        i += 2;
        res.push(StringFragment::Inner(
            chars[open + 1..close].iter().collect::<String>(),
        ));
    }
    if pairs.is_empty() {
        res.push(StringFragment::Outer(chars.into_iter().collect::<String>()));
    } else if pairs[pairs.len() - 1] != chars.len() - 1 {
        res.push(StringFragment::Outer(
            chars[pairs[pairs.len() - 1] + 1..]
                .iter()
                .collect::<String>(),
        ));
    }
    res
}

pub fn color_quotes(msg: &str) -> String {
    let q;
    let styled_msg = if msg.contains('`') {
        q = "`";
        split_quotes_with(msg, '`')
    } else {
        q = "'";
        split_quotes_with(msg, '\'')
    };
    let mut res = "".to_owned();
    for m in styled_msg {
        match m {
            StringFragment::Outer(m) => {
                res += &m;
            }
            StringFragment::Inner(m) => {
                res += q;
                res += &Style::new().yellow().apply_to(&m).to_string();
                res += q;
            }
        }
    }
    res
}

pub fn pretty_title(title: &str, description: &str) -> String {
    format!("{} {}", BLUE.apply_to(title), BOLD.apply_to(description))
}

// ------------------------------------------------------------------------------------------------
// progress

pub fn pretty_green<T: Display>(action: &str, target: T, description: Option<&str>) -> String {
    match description {
        Some(desc) if !desc.is_empty() => {
            format!("{} {} ({})", GREEN.apply_to(action), target, desc)
        }
        _ => format!("{} {}", GREEN.apply_to(action), target),
    }
}
pub fn pretty_yellow<T: Display>(action: &str, target: T, description: Option<&str>) -> String {
    match description {
        Some(desc) if !desc.is_empty() => {
            format!("{} {} ({})", YELLOW.apply_to(action), target, desc)
        }
        _ => format!("{} {}", YELLOW.apply_to(action), target),
    }
}
pub fn pretty_red<T: Display>(action: &str, target: T, description: Option<&str>) -> String {
    match description {
        Some(desc) if !desc.is_empty() => format!("{} {} ({})", RED.apply_to(action), target, desc),
        _ => format!("{} {}", RED.apply_to(action), target),
    }
}

// ------------------------------------------------------------------------------------------------
// verdicts

pub fn pretty_passed(action: &str, target: &str, description: &str) -> String {
    if description.is_empty() {
        format!("{} {}", GREEN.apply_to(action), BLUE.apply_to(target))
    } else {
        format!(
            "{} {} {}",
            GREEN.apply_to(action),
            BLUE.apply_to(target),
            BOLD.apply_to(description)
        )
    }
}
pub fn pretty_failed(action: &str, target: &str, description: &str) -> String {
    if description.is_empty() {
        format!("{} {}", RED.apply_to(action), BLUE.apply_to(target))
    } else {
        format!(
            "{} {} ({})",
            RED.apply_to(action),
            BLUE.apply_to(target),
            BOLD.apply_to(description)
        )
    }
}
pub fn pretty_warned(action: &str, target: &str, description: &str) -> String {
    if description.is_empty() {
        format!("{} {}", CYAN.apply_to(action), BLUE.apply_to(target))
    } else {
        format!(
            "{} {} {}",
            YELLOW.apply_to(action),
            BLUE.apply_to(target),
            BOLD.apply_to(description)
        )
    }
}
pub fn pretty_bugged(target: &str, description: &str) -> String {
    format!(
        "\n{} {} {}: please report this error to SDF",
        RED.apply_to("[BUG]"),
        BLUE.apply_to(target),
        description
    )
}
pub fn pretty_error(action: &str, target: &str, description: &str) -> String {
    if description.is_empty() {
        format!("{} {}", RED.apply_to(action), target)
    } else {
        format!("{} {} ({})", RED.apply_to(action), target, description)
    }
}
pub fn pretty_info(action: &str, target: &str) -> String {
    format!("{} {}", CYAN.apply_to(action), target)
}
pub fn pretty_warning(action: &str, description: &str) -> String {
    format!("{} {}", YELLOW.apply_to(action), color_quotes(description))
}
pub fn pretty_prompt(msg: &str, color: Option<&Style>) -> String {
    let color = color.unwrap_or(&BOLD);
    format!("{}", color.apply_to(msg))
}
pub fn pretty_progress_censured(action: &str, target: &str, censored: &str) -> (String, String) {
    (
        format!(
            "{} {} {}",
            GREEN.apply_to(action),
            color_quotes(target),
            color_quotes(censored)
        ),
        format!(
            "{} {} {}",
            GREEN.apply_to(action),
            color_quotes(target),
            "*".repeat(censored.len())
        ),
    )
}

static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*m").expect("valid regex"));

pub fn remove_ansi_codes(input: &str) -> String {
    RE.replace_all(input, "").to_string()
}

pub fn make_title(title: &str, description: &str) -> String {
    format!("{} {}", BLUE.apply_to(title), BOLD.apply_to(description))
}

pub fn make_error_title(title: &str, description: &str) -> String {
    if description.is_empty() {
        return format!("{}", RED.apply_to(title));
    }
    format!("{} {}", RED.apply_to(title), BOLD.apply_to(description))
}
