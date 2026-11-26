/// A Snowflake stage read in `FROM`, for example
/// `@db.sch.stg/path/ (PATTERN => '.*csv')`.
///
/// It keeps what selects the files read: the location and the `PATTERN`
/// option. Other options, such as `FILE_FORMAT`, are dropped.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StageReference {
    /// The location as written, starting with `@`: the stage name and an
    /// optional path. A location written as a string (`'@stg/my folder/'`)
    /// has its escape sequences expanded and no quotes.
    pub location: String,
    /// The value of the `PATTERN` option, with escape sequences expanded.
    pub pattern: Option<String>,
}
