#![allow(clippy::cognitive_complexity)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::if_same_then_else)]
#![allow(clippy::let_and_return)]
#![allow(clippy::needless_bool)]
#![allow(clippy::only_used_in_recursion)]
#![allow(clippy::should_implement_trait)]

pub mod ident;
pub mod statements;
pub mod tokenizer;
pub mod types;

mod keywords;
pub use keywords::is_keyword_ignore_ascii_case;
