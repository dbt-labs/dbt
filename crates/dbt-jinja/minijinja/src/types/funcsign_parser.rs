use std::{collections::BTreeMap, sync::Arc};

use dashmap::DashMap;

use crate::types::{
    builtins::Reference, dict::DictType, function::LambdaType, list::ListType, struct_::StructType,
    tuple::TupleType, union::UnionType, DynObject, Type,
};

/// The error type for the funcsign parser.
#[derive(Debug, Clone)]
pub struct ParseError {
    /// The error message.
    pub message: String,
    /// The location of the error.
    pub location: CodeLocation,
}

impl ParseError {
    /// Create a new parse error.
    pub fn new(message: String, location: CodeLocation) -> Self {
        Self { message, location }
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Parse error at line {}, column {}: {}",
            self.location.line, self.location.col, self.message
        )
    }
}

impl std::error::Error for ParseError {}

/// The location of a token in the funcsign.
#[derive(Debug, Clone)]
pub struct CodeLocation {
    /// The line number of the token.
    pub line: u32,
    /// The column number of the token.
    pub col: u32,
    /// The index of the token.
    #[allow(dead_code)]
    pub index: u32,
}

impl Default for CodeLocation {
    fn default() -> Self {
        Self {
            line: 1,
            col: 1,
            index: 0,
        }
    }
}

fn get_token_location(tokens: &[Token], index: usize) -> CodeLocation {
    tokens
        .get(index)
        .and_then(|t| t.location())
        .unwrap_or_default()
}

#[derive(Debug)]
enum Token {
    OpenParen(CodeLocation),
    CloseParen(CodeLocation),
    OpenBracket(CodeLocation),
    CloseBracket(CodeLocation),
    OpenBrace(CodeLocation),
    CloseBrace(CodeLocation),
    Comma(CodeLocation),
    Arrow(CodeLocation),
    Colon(CodeLocation),
    Pipe(CodeLocation),
    Identifier(String, CodeLocation),
}

impl Token {
    fn location(&self) -> Option<CodeLocation> {
        match self {
            Token::OpenParen(loc) => Some(loc.clone()),
            Token::CloseParen(loc) => Some(loc.clone()),
            Token::OpenBracket(loc) => Some(loc.clone()),
            Token::CloseBracket(loc) => Some(loc.clone()),
            Token::OpenBrace(loc) => Some(loc.clone()),
            Token::CloseBrace(loc) => Some(loc.clone()),
            Token::Comma(loc) => Some(loc.clone()),
            Token::Arrow(loc) => Some(loc.clone()),
            Token::Colon(loc) => Some(loc.clone()),
            Token::Pipe(loc) => Some(loc.clone()),
            Token::Identifier(_, loc) => Some(loc.clone()),
        }
    }
}

fn tokenize(s: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut current_token = String::new();
    let mut line = 1u32;
    let mut col = 1u32;
    let mut index = 0u32;

    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        if c.is_whitespace() {
            if !current_token.is_empty() {
                tokens.push(Token::Identifier(
                    current_token.clone(),
                    CodeLocation { line, col, index },
                ));
                current_token = String::new();
            }
            if c == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        } else if c == '(' {
            if !current_token.is_empty() {
                tokens.push(Token::Identifier(
                    current_token.clone(),
                    CodeLocation { line, col, index },
                ));
                current_token = String::new();
            }
            tokens.push(Token::OpenParen(CodeLocation { line, col, index }));
            col += 1;
        } else if c == ')' {
            if !current_token.is_empty() {
                tokens.push(Token::Identifier(
                    current_token.clone(),
                    CodeLocation { line, col, index },
                ));
                current_token = String::new();
            }
            tokens.push(Token::CloseParen(CodeLocation { line, col, index }));
            col += 1;
        } else if c == '[' {
            if !current_token.is_empty() {
                tokens.push(Token::Identifier(
                    current_token.clone(),
                    CodeLocation { line, col, index },
                ));
                current_token = String::new();
            }
            tokens.push(Token::OpenBracket(CodeLocation { line, col, index }));
            col += 1;
        } else if c == ']' {
            if !current_token.is_empty() {
                tokens.push(Token::Identifier(
                    current_token.clone(),
                    CodeLocation { line, col, index },
                ));
                current_token = String::new();
            }
            tokens.push(Token::CloseBracket(CodeLocation { line, col, index }));
            col += 1;
        } else if c == '{' {
            if !current_token.is_empty() {
                tokens.push(Token::Identifier(
                    current_token.clone(),
                    CodeLocation { line, col, index },
                ));
                current_token = String::new();
            }
            tokens.push(Token::OpenBrace(CodeLocation { line, col, index }));
            col += 1;
        } else if c == '}' {
            if !current_token.is_empty() {
                tokens.push(Token::Identifier(
                    current_token.clone(),
                    CodeLocation { line, col, index },
                ));
                current_token = String::new();
            }
            tokens.push(Token::CloseBrace(CodeLocation { line, col, index }));
            col += 1;
        } else if c == ',' {
            if !current_token.is_empty() {
                tokens.push(Token::Identifier(
                    current_token.clone(),
                    CodeLocation { line, col, index },
                ));
                current_token = String::new();
            }
            tokens.push(Token::Comma(CodeLocation { line, col, index }));
            col += 1;
        } else if c == '-' && i + 1 < chars.len() && chars[i + 1] == '>' {
            if !current_token.is_empty() {
                tokens.push(Token::Identifier(
                    current_token.clone(),
                    CodeLocation { line, col, index },
                ));
                current_token = String::new();
            }
            tokens.push(Token::Arrow(CodeLocation { line, col, index }));
            i += 1; // Skip the '>' character
            col += 2;
        } else if c == ':' {
            if !current_token.is_empty() {
                tokens.push(Token::Identifier(
                    current_token.clone(),
                    CodeLocation { line, col, index },
                ));
                current_token = String::new();
            }
            tokens.push(Token::Colon(CodeLocation { line, col, index }));
            col += 1;
        } else if c == '|' {
            if !current_token.is_empty() {
                tokens.push(Token::Identifier(
                    current_token.clone(),
                    CodeLocation { line, col, index },
                ));
                current_token = String::new();
            }
            tokens.push(Token::Pipe(CodeLocation { line, col, index }));
            col += 1;
        } else {
            current_token.push(c);
            col += 1;
        }

        i += 1;
        index += 1;
    }

    if !current_token.is_empty() {
        tokens.push(Token::Identifier(
            current_token.clone(),
            CodeLocation { line, col, index },
        ));
    }
    tokens
}

fn _parse_type(
    tokens: &[Token],
    index: usize,
    registry: Arc<DashMap<String, Type>>,
) -> Result<(Type, usize), ParseError> {
    let next_token = tokens.get(index);
    let (type_, consumed) = match next_token {
        Some(Token::OpenParen(_)) => {
            let (args, ret_type, consumed) = parse_lambda(tokens, index, registry.clone())?;
            Ok((
                Type::Object(DynObject::new(Arc::new(LambdaType::new(args, ret_type)))),
                consumed,
            ))
        }
        Some(Token::Identifier(id, _location)) => match id.as_str() {
            "string" => Ok((Type::String(None), 1)),
            "integer" => Ok((Type::Integer(None), 1)),
            "float" => Ok((Type::Float, 1)),
            "bool" => Ok((Type::Bool, 1)),
            "bytes" => Ok((Type::Bytes, 1)),
            "seq" | "list" => {
                let (parameters, consumed) = parse_list(tokens, index + 1, registry.clone())?;
                if parameters.len() != 1 {
                    let location = get_token_location(tokens, index);
                    Err(ParseError::new(
                        "Expected 1 parameter for list type".to_string(),
                        location,
                    ))
                } else {
                    Ok((
                        Type::List(ListType::new(parameters[0].clone())),
                        1 + consumed,
                    ))
                }
            }
            "dict" => {
                let (parameters, consumed) = parse_list(tokens, index + 1, registry.clone())?;
                if parameters.len() != 2 {
                    let location = get_token_location(tokens, index);
                    Err(ParseError::new(
                        "Expected 2 parameters for dict type".to_string(),
                        location,
                    ))
                } else {
                    Ok((
                        Type::Dict(DictType::new(parameters[0].clone(), parameters[1].clone())),
                        1 + consumed,
                    ))
                }
            }
            "tuple" => {
                let (elements, consumed) = parse_list(tokens, index + 1, registry.clone())?;
                Ok((Type::Tuple(TupleType::new(elements)), 1 + consumed))
            }
            "struct" => {
                let (fields, consumed) = parse_fields(tokens, index + 1, registry.clone())?;
                Ok((Type::Struct(StructType::new(fields)), 1 + consumed))
            }
            "optional" => {
                let (parameters, consumed) = parse_list(tokens, index + 1, registry.clone())?;
                if parameters.len() != 1 {
                    let location = get_token_location(tokens, index);
                    Err(ParseError::new(
                        "Expected 1 parameter for optional type".to_string(),
                        location,
                    ))
                } else {
                    Ok((
                        Type::Union(UnionType::new(vec![parameters[0].clone(), Type::None])),
                        1 + consumed,
                    ))
                }
            }
            "none" => Ok((Type::None, 1)),
            "any" => Ok((Type::Any { hard: false }, 1)),
            "ANY" => Ok((Type::Any { hard: true }, 1)),
            "timestamp" => Ok((Type::TimeStamp, 1)),

            _ => {
                let builtin = Reference::new(id.clone(), registry.clone());
                let type_ = Type::Object(DynObject::new(Arc::new(builtin)));
                Ok((type_, 1))
            }
        },
        None => Err(ParseError::new(
            "Unexpected end of input".to_string(),
            get_token_location(tokens, index),
        )),
        _ => Err(ParseError::new(
            "Unexpected token".to_string(),
            get_token_location(tokens, index),
        )),
    }?;
    if let Some(Token::Pipe(_)) = tokens.get(index + consumed) {
        let (or_type, or_consumed) = _parse_type(tokens, index + consumed + 1, registry)?;
        Ok((type_.union(&or_type), consumed + or_consumed + 1))
    } else {
        Ok((type_, consumed))
    }
}

fn parse_lambda(
    tokens: &[Token],
    mut index: usize,
    registry: Arc<DashMap<String, Type>>,
) -> Result<(Vec<Type>, Type, usize), ParseError> {
    let start_index = index;
    let mut args = Vec::new();

    // Expect opening parenthesis
    match tokens.get(index) {
        Some(Token::OpenParen(_)) => {
            index += 1; // Skip opening paren
        }
        _ => {
            return Err(ParseError::new(
                "Expected opening parenthesis".to_string(),
                get_token_location(tokens, index),
            ))
        }
    }

    // Parse arguments
    if let Some(Token::CloseParen(_)) = tokens.get(index) {
        // Empty parameter list
        index += 1; // Skip closing paren
    } else {
        // Parse first argument
        let (arg_type, consumed) = _parse_type(tokens, index, registry.clone())?;
        args.push(arg_type);
        index += consumed;

        // Parse remaining arguments
        while let Some(token) = tokens.get(index) {
            match token {
                Token::CloseParen(_) => {
                    index += 1; // Skip closing paren
                    break;
                }
                Token::Comma(_) => {
                    index += 1; // Skip comma
                    let (arg_type, consumed) = _parse_type(tokens, index, registry.clone())?;
                    args.push(arg_type);
                    index += consumed;
                }
                _ => {
                    return Err(ParseError::new(
                        "Expected ',' or ')' in parameter list".to_string(),
                        get_token_location(tokens, index),
                    ))
                }
            }
        }
    }

    // Parse arrow and return type - NOW REQUIRED for function signatures
    let ret_type = match tokens.get(index) {
        Some(Token::Arrow(_)) => {
            index += 1; // Skip arrow
            let (ret_type, consumed) = _parse_type(tokens, index, registry)?;
            index += consumed;
            ret_type
        }
        Some(Token::Identifier(_, _)) => {
            // If we see an identifier after the closing paren without an arrow, it's an error
            return Err(ParseError::new(
                "Expected '->' before return type".to_string(),
                get_token_location(tokens, index),
            ));
        }
        _ => Type::None,
    };

    Ok((args, ret_type, index - start_index))
}

fn parse_list(
    tokens: &[Token],
    mut index: usize,
    registry: Arc<DashMap<String, Type>>,
) -> Result<(Vec<Type>, usize), ParseError> {
    let start_index = index;
    match tokens.get(index) {
        Some(Token::OpenBracket(_)) => {
            index += 1; // Skip opening bracket
            let mut parameters = Vec::new();

            if let Some(Token::CloseBracket(_)) = tokens.get(index) {
                // Empty parameter list
                index += 1; // Skip closing bracket
                return Ok((parameters, index - start_index));
            }

            // Parse first parameter
            let (parameter, consumed) = _parse_type(tokens, index, registry.clone())?;
            parameters.push(parameter);
            index += consumed;

            // Parse remaining parameters
            loop {
                match tokens.get(index) {
                    Some(Token::CloseBracket(_)) => {
                        index += 1; // Skip closing bracket
                        break;
                    }
                    Some(Token::Comma(_)) => {
                        index += 1; // Skip comma
                        let (parameter, consumed) = _parse_type(tokens, index, registry.clone())?;
                        parameters.push(parameter);
                        index += consumed;
                    }
                    Some(_) => {
                        return Err(ParseError::new(
                            "Expected ',' or ']' in parameter list".to_string(),
                            get_token_location(tokens, index),
                        ))
                    }
                    None => {
                        return Err(ParseError::new(
                            "Unclosed bracket - expected ']'".to_string(),
                            CodeLocation::default(),
                        ))
                    }
                }
            }
            Ok((parameters, index - start_index))
        }
        _ => {
            let location = get_token_location(tokens, index);
            Err(ParseError::new(
                "Expected open bracket for list type".to_string(),
                location,
            ))
        }
    }
}

fn parse_fields(
    tokens: &[Token],
    mut index: usize,
    registry: Arc<DashMap<String, Type>>,
) -> Result<(BTreeMap<String, Type>, usize), ParseError> {
    let start_index = index;
    let mut fields = BTreeMap::new();

    match tokens.get(index) {
        Some(Token::OpenBrace(_)) => {
            index += 1; // Skip opening brace

            if let Some(Token::CloseBrace(_)) = tokens.get(index) {
                // Empty field list
                index += 1; // Skip closing brace
                return Ok((fields, index - start_index));
            }

            // Parse first field
            let field_name = match tokens.get(index) {
                Some(Token::Identifier(name, _)) => {
                    index += 1; // Skip field name
                    name.clone()
                }
                _ => {
                    return Err(ParseError::new(
                        "Expected field name".to_string(),
                        get_token_location(tokens, index),
                    ))
                }
            };

            // Expect colon
            match tokens.get(index) {
                Some(Token::Colon(_)) => {
                    index += 1; // Skip colon
                }
                _ => {
                    return Err(ParseError::new(
                        "Expected ':' after field name".to_string(),
                        get_token_location(tokens, index),
                    ))
                }
            }

            let (field_type, consumed) = _parse_type(tokens, index, registry.clone())?;
            fields.insert(field_name, field_type);
            index += consumed;

            // Parse remaining fields
            loop {
                match tokens.get(index) {
                    Some(Token::CloseBrace(_)) => {
                        index += 1; // Skip closing brace
                        break;
                    }
                    Some(Token::Comma(_)) => {
                        index += 1; // Skip comma

                        let field_name = match tokens.get(index) {
                            Some(Token::Identifier(name, _)) => {
                                index += 1; // Skip field name
                                name.clone()
                            }
                            _ => {
                                return Err(ParseError::new(
                                    "Expected field name".to_string(),
                                    get_token_location(tokens, index),
                                ))
                            }
                        };

                        // Expect colon
                        match tokens.get(index) {
                            Some(Token::Colon(_)) => {
                                index += 1; // Skip colon
                            }
                            _ => {
                                return Err(ParseError::new(
                                    "Expected ':' after field name".to_string(),
                                    get_token_location(tokens, index),
                                ))
                            }
                        }

                        let (field_type, consumed) = _parse_type(tokens, index, registry.clone())?;
                        fields.insert(field_name, field_type);
                        index += consumed;
                    }
                    Some(_) => {
                        return Err(ParseError::new(
                            "Expected ',' or '}' in field list".to_string(),
                            get_token_location(tokens, index),
                        ))
                    }
                    None => {
                        return Err(ParseError::new(
                            "Unclosed brace - expected '}'".to_string(),
                            CodeLocation::default(),
                        ))
                    }
                }
            }
            Ok((fields, index - start_index))
        }
        _ => {
            let location = get_token_location(tokens, index);
            Err(ParseError::new(
                "Expected open brace for struct type".to_string(),
                location,
            ))
        }
    }
}

/// Parse a funcsign string into a function signature.
///
/// # Arguments
///
/// * `s` - The funcsign string to parse.
/// * `registry` - The registry of types to use for parsing.
///
/// # Returns
///
/// A tuple containing the arguments and return type of the function.
pub fn parse(
    s: &str,
    registry: Arc<DashMap<String, Type>>,
) -> Result<(Vec<Type>, Type), ParseError> {
    let tokens = tokenize(s);
    let (args, ret_type, _) = parse_lambda(&tokens, 0, registry)?;
    Ok((args, ret_type))
}

pub(crate) fn parse_type(
    s: &str,
    registry: Arc<DashMap<String, Type>>,
) -> Result<Type, ParseError> {
    let tokens = tokenize(s);
    let (type_, _) = _parse_type(&tokens, 0, registry)?;
    Ok(type_)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_function() {
        let registry = Arc::new(DashMap::new());
        let (args, ret_type) = parse("() -> string", registry).unwrap();
        assert_eq!(args.len(), 0);
        matches!(ret_type, Type::String(None));
    }

    #[test]
    fn test_parse_function_with_single_arg() {
        let registry = Arc::new(DashMap::new());
        let (args, ret_type) = parse("(string) -> integer", registry).unwrap();
        assert_eq!(args.len(), 1);
        matches!(args[0], Type::String(None));
        matches!(ret_type, Type::Integer(None));
    }

    #[test]
    fn test_parse_function_with_multiple_args() {
        let registry = Arc::new(DashMap::new());
        let (args, ret_type) = parse("(string, integer, bool) -> float", registry).unwrap();
        assert_eq!(args.len(), 3);
        matches!(args[0], Type::String(None));
        matches!(args[1], Type::Integer(None));
        matches!(args[2], Type::Bool);
        matches!(ret_type, Type::Float);
    }

    #[test]
    fn test_parse_function_returning_list() {
        let registry = Arc::new(DashMap::new());
        let (args, ret_type) = parse("(string) -> seq[integer]", registry).unwrap();
        assert_eq!(args.len(), 1);
        matches!(args[0], Type::String(None));
        matches!(ret_type, Type::List { .. });
    }

    #[test]
    fn test_parse_function_returning_dict() {
        let registry = Arc::new(DashMap::new());
        let (args, ret_type) = parse("(string) -> dict[string, integer]", registry).unwrap();
        assert_eq!(args.len(), 1);
        matches!(args[0], Type::String(None));
        matches!(ret_type, Type::Dict { .. });
    }

    #[test]
    fn test_parse_function_with_list_args() {
        let registry = Arc::new(DashMap::new());
        let (args, ret_type) = parse("(seq[string], list[integer]) -> bool", registry).unwrap();
        assert_eq!(args.len(), 2);
        matches!(args[0], Type::List { .. });
        matches!(args[1], Type::List { .. });
        matches!(ret_type, Type::Bool);
    }

    #[test]
    fn test_parse_function_with_dict_args() {
        let registry = Arc::new(DashMap::new());
        let (args, ret_type) = parse(
            "(dict[string, integer], dict[integer, bool]) -> string",
            registry,
        )
        .unwrap();
        assert_eq!(args.len(), 2);
        matches!(args[0], Type::Dict { .. });
        matches!(args[1], Type::Dict { .. });
        matches!(ret_type, Type::String(None));
    }

    #[test]
    fn test_parse_function_with_struct_args() {
        let registry = Arc::new(DashMap::new());
        let (args, ret_type) =
            parse("(struct{name: string, age: integer}) -> bool", registry).unwrap();
        assert_eq!(args.len(), 1);
        matches!(args[0], Type::Struct(_));
        matches!(ret_type, Type::Bool);
    }

    #[test]
    fn test_parse_function_returning_struct() {
        let registry = Arc::new(DashMap::new());
        let (args, ret_type) = parse(
            "(string, integer) -> struct{id: integer, name: string, active: bool}",
            registry,
        )
        .unwrap();
        assert_eq!(args.len(), 2);
        matches!(args[0], Type::String(None));
        matches!(args[1], Type::Integer(None));
        matches!(ret_type, Type::Struct(_));
    }

    #[test]
    fn test_parse_function_with_class_types() {
        let registry = Arc::new(DashMap::new());
        let (args, ret_type) = parse("(relation, adapter) -> api", registry).unwrap();
        assert_eq!(args.len(), 2);
        matches!(args[0], Type::Object(_));
        matches!(args[1], Type::Object(_));
        matches!(ret_type, Type::Object(_));
    }

    #[test]
    fn test_parse_function_with_nested_function_type() {
        let registry = Arc::new(DashMap::new());
        let (args, ret_type) = parse("((string) -> integer, string) -> bool", registry).unwrap();
        assert_eq!(args.len(), 2);
        matches!(args[0], Type::Object(_));
        matches!(args[1], Type::String(None));
        matches!(ret_type, Type::Bool);
    }

    #[test]
    fn test_parse_function_with_complex_nested_types() {
        let registry = Arc::new(DashMap::new());
        let (args, ret_type) = parse(
            "(seq[dict[string, integer]], struct{users: seq[string]}) -> dict[string, bool]",
            registry,
        )
        .unwrap();
        assert_eq!(args.len(), 2);
        matches!(args[0], Type::List { .. });
        matches!(args[1], Type::Struct(_));
        matches!(ret_type, Type::Dict { .. });
    }

    #[test]
    fn test_parse_function_with_all_primitive_types() {
        let registry = Arc::new(DashMap::new());
        let (args, ret_type) =
            parse("(string, integer, float, bool, bytes) -> string", registry).unwrap();
        assert_eq!(args.len(), 5);
        matches!(args[0], Type::String(None));
        matches!(args[1], Type::Integer(None));
        matches!(args[2], Type::Float);
        matches!(args[3], Type::Bool);
        matches!(args[4], Type::Bytes);
        matches!(ret_type, Type::String(None));
    }

    #[test]
    fn test_parse_function_with_whitespace() {
        let registry = Arc::new(DashMap::new());
        let (args, ret_type) = parse("  ( string , integer )  ->  bool  ", registry).unwrap();
        assert_eq!(args.len(), 2);
        matches!(args[0], Type::String(None));
        matches!(args[1], Type::Integer(None));
        matches!(ret_type, Type::Bool);
    }

    #[test]
    fn test_parse_function_returning_apicolumn() {
        let registry = Arc::new(DashMap::new());
        let (args, ret_type) = parse("(string) -> api_column", registry).unwrap();
        assert_eq!(args.len(), 1);
        matches!(args[0], Type::String(None));
        matches!(ret_type, Type::Object(_));
    }

    #[test]
    fn test_parse_complex_function_signature() {
        let registry = Arc::new(DashMap::new());
        let (args, ret_type) = parse("(relation, dict[string, list[string]], (relation, string, list[string]) -> string) -> list[string]", registry).unwrap();

        // Should have 3 arguments
        assert_eq!(args.len(), 3);

        // First arg should be relation (class type)
        matches!(args[0], Type::Object(_));

        // Second arg should be dict[string, list[string]]
        matches!(args[1], Type::Dict { .. });
        if let Type::Dict(dict) = &args[1] {
            matches!(*dict.key, Type::String(None));
            if let Type::List(list) = &*dict.value {
                matches!(*list.element, Type::String(None));
            } else {
                panic!("Expected seq type for dict value");
            }
        } else {
            panic!("Expected dict type for second argument");
        }

        // Third arg should be a function type: (relation, string, list[string]) -> string
        matches!(args[2], Type::Object(_));
        if let Type::Object(func) = &args[2] {
            let func = func.downcast_ref::<LambdaType>().unwrap();

            // Function should have 3 args: relation, string, list[string]
            assert_eq!(func.args.len(), 3);
            matches!(func.args[0], Type::Object(_)); // relation
            matches!(func.args[1], Type::String(None)); // string
            matches!(func.args[2], Type::List { .. }); // list[string]

            // Function return type should be string
            matches!(func.ret_type, Type::String(None));
        } else {
            panic!("Expected function type for third argument");
        }

        // Return type should be list[string]
        matches!(ret_type, Type::List { .. });
        if let Type::List(list) = &ret_type {
            matches!(*list.element, Type::String(None));
        } else {
            panic!("Expected seq type for return value");
        }
    }

    // Tests for error conditions
    #[test]
    fn test_parse_invalid_syntax() {
        let registry = Arc::new(DashMap::new());
        let result = parse("invalid -> -> string", registry);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("Unexpected token") || error.message.contains("Expected"));
    }

    #[test]
    fn test_parse_missing_return_type() {
        let registry = Arc::new(DashMap::new());
        let result = parse("(string) ->", registry);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(
            error.message.contains("Unexpected end of input") || error.message.contains("Expected")
        );
    }

    #[test]
    fn test_parse_missing_arrow() {
        let registry = Arc::new(DashMap::new());
        let result = parse("(string) string", registry);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("Unexpected token") || error.message.contains("Expected"));
    }

    #[test]
    fn test_parse_unclosed_parenthesis() {
        let registry = Arc::new(DashMap::new());
        let result = parse("(string -> bool", registry);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(
            error.message.contains("Unexpected end of input") || error.message.contains("Expected")
        );
    }

    #[test]
    fn test_parse_unclosed_bracket() {
        let registry = Arc::new(DashMap::new());
        let result = parse("(seq[string) -> bool", registry);
        assert!(result.is_err());
        let error = result.unwrap_err();
        println!("Actual error message: '{}'", error.message);
        assert!(error
            .message
            .contains("Expected ',' or ']' in parameter list"));
    }

    #[test]
    fn test_parse_unclosed_brace() {
        let registry = Arc::new(DashMap::new());
        let result = parse("(struct{name: string) -> bool", registry);
        assert!(result.is_err());
        let error = result.unwrap_err();
        println!("Actual error message: '{}'", error.message);
        assert!(error.message.contains("Expected ',' or '}' in field list"));
    }

    // New error handling tests
    #[test]
    fn test_parse_error_with_location() {
        let registry = Arc::new(DashMap::new());
        let result = parse("(unknown_bracket_type[) -> bool", registry);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.location.line > 0);
        assert!(error.location.col > 0);
    }

    #[test]
    fn test_parse_error_message_display() {
        let registry = Arc::new(DashMap::new());
        let result = parse("(invalid -> bool", registry);
        assert!(result.is_err());
        let error = result.unwrap_err();
        let error_str = format!("{error}");
        assert!(error_str.contains("Parse error"));
    }

    #[test]
    fn test_parse_empty_input() {
        let registry = Arc::new(DashMap::new());
        let result = parse("", registry);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("Expected opening parenthesis"));
    }

    #[test]
    fn test_parse_malformed_list_type() {
        let registry = Arc::new(DashMap::new());
        let result = parse("(seq[string, integer, bool) -> bool", registry);
        assert!(result.is_err());
        let error = result.unwrap_err();
        println!("Actual error message: '{}'", error.message);
        assert!(error
            .message
            .contains("Expected ',' or ']' in parameter list"));
    }

    #[test]
    fn test_parse_malformed_dict_type() {
        let registry = Arc::new(DashMap::new());
        let result = parse("(dict[string) -> bool", registry);
        assert!(result.is_err());
        let error = result.unwrap_err();
        println!("Actual error message: '{}'", error.message);
        assert!(error
            .message
            .contains("Expected ',' or ']' in parameter list"));
    }

    // Tests for union type parsing
    #[test]
    fn test_parse_simple_union_type() {
        let registry = Arc::new(DashMap::new());
        let result = parse_type("string | integer", registry).unwrap();
        matches!(result, Type::Union(_));
    }

    #[test]
    fn test_parse_multiple_union_types() {
        let registry = Arc::new(DashMap::new());
        let result = parse_type("string | integer | bool", registry).unwrap();
        matches!(result, Type::Union(_));
    }

    #[test]
    fn test_parse_function_with_union_parameter() {
        let registry = Arc::new(DashMap::new());
        let (args, ret_type) = parse("(string | integer) -> bool", registry).unwrap();
        assert_eq!(args.len(), 1);
        matches!(args[0], Type::Union(_));
        matches!(ret_type, Type::Bool);
    }

    #[test]
    fn test_parse_function_with_union_return_type() {
        let registry = Arc::new(DashMap::new());
        let (args, ret_type) = parse("(string) -> integer | bool", registry).unwrap();
        assert_eq!(args.len(), 1);
        matches!(args[0], Type::String(None));
        matches!(ret_type, Type::Union(_));
    }

    #[test]
    fn test_parse_complex_union_with_lists() {
        let registry = Arc::new(DashMap::new());
        let result = parse_type("seq[string] | seq[integer]", registry).unwrap();
        matches!(result, Type::Union(_));
    }

    #[test]
    fn test_parse_complex_union_with_dicts() {
        let registry = Arc::new(DashMap::new());
        let result = parse_type("dict[string, integer] | dict[string, bool]", registry).unwrap();
        matches!(result, Type::Union(_));
    }

    #[test]
    fn test_parse_union_with_struct() {
        let registry = Arc::new(DashMap::new());
        let result = parse_type("struct{name: string} | struct{id: integer}", registry).unwrap();
        matches!(result, Type::Union(_));
    }

    #[test]
    fn test_parse_union_with_custom_types() {
        let registry = Arc::new(DashMap::new());
        let result = parse_type("relation | adapter", registry).unwrap();
        matches!(result, Type::Union(_));
    }

    #[test]
    fn test_parse_function_with_multiple_union_parameters() {
        let registry = Arc::new(DashMap::new());
        let (args, ret_type) = parse("(string | integer, bool | float) -> any", registry).unwrap();
        assert_eq!(args.len(), 2);
        matches!(args[0], Type::Union(_));
        matches!(args[1], Type::Union(_));
        matches!(ret_type, Type::Any { .. });
    }

    #[test]
    fn test_parse_union_with_optional() {
        let registry = Arc::new(DashMap::new());
        let result = parse_type("string | optional[integer]", registry).unwrap();
        matches!(result, Type::Union(_));
    }

    #[test]
    fn test_parse_union_with_whitespace() {
        let registry = Arc::new(DashMap::new());
        let result = parse_type("string  |  integer  |  bool", registry).unwrap();
        matches!(result, Type::Union(_));
    }
}
