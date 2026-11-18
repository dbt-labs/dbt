use console::Style;
use std::fmt::Display;
use std::sync::LazyLock;

pub static CYAN: LazyLock<Style> = LazyLock::new(|| Style::new().cyan().bold());
pub static BLUE: LazyLock<Style> = LazyLock::new(|| Style::new().blue().bold());
pub static RED: LazyLock<Style> = LazyLock::new(|| Style::new().red().bold());
pub static YELLOW: LazyLock<Style> = LazyLock::new(|| Style::new().yellow().bold());
pub static GREEN: LazyLock<Style> = LazyLock::new(|| Style::new().green().bold());
pub static BOLD: LazyLock<Style> = LazyLock::new(|| Style::new().bold());
pub static MAGENTA: LazyLock<Style> = LazyLock::new(|| Style::new().magenta().bold());
pub static DIM: LazyLock<Style> = LazyLock::new(|| Style::new().dim());
pub static WHITE: LazyLock<Style> = LazyLock::new(|| Style::new().white().bold());
pub static ITALIC: LazyLock<Style> = LazyLock::new(|| Style::new().italic());
pub static PLAIN: LazyLock<Style> = LazyLock::new(Style::default);

/// Displays `msg` with text between quotes highlighted in yellow.
///
/// Quotes are backticks if `msg` contains any, single quotes otherwise. "doesn't" is
/// written as "does not", so its apostrophe does not open a quote. If the quotes are
/// unbalanced, `msg` is displayed unchanged.
pub struct ColoredQuotes<'a>(pub &'a str);

impl ColoredQuotes<'_> {
    /// Byte offsets of the quotes in `self.0`.
    fn quote_positions(&self) -> impl Iterator<Item = usize> + '_ {
        let msg = self.0;
        let quote = if msg.contains('`') { '`' } else { '\'' };
        msg.match_indices(quote)
            .map(|(index, _)| index)
            .filter(move |&index| {
                quote == '`'
                    || !(msg[..index].ends_with("doesn") && msg[index + 1..].starts_with('t'))
            })
    }

    /// Displays `msg` like [`Display`], but highlights quotes irrespective of the global
    /// auto-detected `console` setting.
    pub(crate) fn fmt_forced(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.fmt_with(f, Style::new().yellow().force_styling(true))
    }

    fn fmt_with(&self, f: &mut std::fmt::Formatter<'_>, quoted: Style) -> std::fmt::Result {
        let msg = self.0;
        if !self.quote_positions().count().is_multiple_of(2) {
            // This gets called with various content which we can't predict-
            // for example, human-readable error messages. For now, let's play it
            // safe by returning the original string (rather than maybe losing
            // error messages).
            //
            // TODO: This approach would still apply inappropriate colors to e.g.
            // "I don't like Randy's TODO" (note that two single quotes appear)
            // We should probably use a less likely text marker, like ``, or
            // use better types.
            return f.write_str(msg);
        }

        let mut positions = self.quote_positions();
        let mut last = 0;
        while let (Some(open), Some(close)) = (positions.next(), positions.next()) {
            let quote = &msg[open..=open];
            write!(
                f,
                "{}{quote}{}{quote}",
                WithDoesntApostropheRemoved(&msg[last..open]),
                quoted.apply_to(WithDoesntApostropheRemoved(&msg[open + 1..close]))
            )?;
            last = close + 1;
        }
        write!(f, "{}", WithDoesntApostropheRemoved(&msg[last..]))
    }
}

impl Display for ColoredQuotes<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.fmt_with(f, Style::new().yellow())
    }
}

/// Displays text with "doesn't" written as "does not".
//TODO Replace DF error messages with proper form, this is a quick HACK
struct WithDoesntApostropheRemoved<'a>(&'a str);

impl Display for WithDoesntApostropheRemoved<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = self.0.split("doesn't");
        if let Some(first) = parts.next() {
            f.write_str(first)?;
        }
        for part in parts {
            f.write_str("does not")?;
            f.write_str(part)?;
        }
        Ok(())
    }
}

pub fn pretty_title(title: &str, description: &str) -> String {
    format!("{} {}", BLUE.apply_to(title), BOLD.apply_to(description))
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
