/*
 * MiniJinja for C
 *
 * Copyright 2024 Armin Ronacher. MIT License.
 */

#ifndef _minijinja_h_included
#define _minijinja_h_included

#pragma once

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

#ifndef MINIJINJA_API
#  define MINIJINJA_API
#endif


/*
 The kind of error that occurred.
 */
typedef enum mj_err_kind {
  MJ_ERR_KIND_NON_PRIMITIVE,
  MJ_ERR_KIND_NON_KEY,
  MJ_ERR_KIND_INVALID_OPERATION,
  MJ_ERR_KIND_SYNTAX_ERROR,
  MJ_ERR_KIND_TEMPLATE_NOT_FOUND,
  MJ_ERR_KIND_TOO_MANY_ARGUMENTS,
  MJ_ERR_KIND_MISSING_ARGUMENT,
  MJ_ERR_KIND_UNKNOWN_FILTER,
  MJ_ERR_KIND_UNKNOWN_FUNCTION,
  MJ_ERR_KIND_UNKNOWN_TEST,
  MJ_ERR_KIND_UNKNOWN_METHOD,
  MJ_ERR_KIND_BAD_ESCAPE,
  MJ_ERR_KIND_UNDEFINED_ERROR,
  MJ_ERROR_KIND_BAD_SERIALIZTION,
  MJ_ERR_KIND_BAD_INCLUDE,
  MJ_ERR_KIND_EVAL_BLOCK,
  MJ_ERR_KIND_CANNOT_UNPACK,
  MJ_ERR_KIND_WRITE_FAILURE,
  MJ_ERR_KIND_UNKNOWN,
} mj_err_kind;

/*
 Controls the undefined behavior of the engine.
 */
typedef enum mj_undefined_behavior {
  /*
   The default, somewhat lenient undefined behavior.
   */
  MJ_UNDEFINED_BEHAVIOR_LENIENT,
  /*
   Complains very quickly about undefined values.
   */
  MJ_UNDEFINED_BEHAVIOR_STRICT,
  /*
   Like Lenient, but also allows chaining of undefined lookups.
   */
  MJ_UNDEFINED_BEHAVIOR_CHAINABLE,
} mj_undefined_behavior;

/*
 The kind of a value.
 */
typedef enum mj_value_kind {
  MJ_VALUE_KIND_UNDEFINED,
  MJ_VALUE_KIND_NONE,
  MJ_VALUE_KIND_BOOL,
  MJ_VALUE_KIND_NUMBER,
  MJ_VALUE_KIND_STRING,
  MJ_VALUE_KIND_BYTES,
  MJ_VALUE_KIND_SEQ,
  MJ_VALUE_KIND_MAP,
  MJ_VALUE_KIND_ITERABLE,
  MJ_VALUE_KIND_PLAIN,
  MJ_VALUE_KIND_INVALID,
} mj_value_kind;

/*
 Pointer to a MiniJinja environment.
 */
typedef struct mj_env mj_env;

/*
 Helps iterating over a value.
 */
typedef struct mj_value_iter mj_value_iter;

/*
 Opaque value type.
 */
typedef struct mj_value {
  uint64_t _opaque[3];
} mj_value;

/*
 Allows one to override the syntax elements.
 */
typedef struct mj_syntax_config {
  const char *block_start;
  const char *block_end;
  const char *variable_start;
  const char *variable_end;
  const char *comment_start;
  const char *comment_end;
  const char *line_statement_prefix;
  const char *line_comment_prefix;
} mj_syntax_config;

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

/*
 Registers a template with the environment.
 */
MINIJINJA_API bool mj_env_add_template(struct mj_env *env, const char *name, const char *source);

/*
 Clears all templates.
 */
MINIJINJA_API bool mj_env_clear_templates(struct mj_env *env);

/*
 Evaluate an expression.
 */
MINIJINJA_API
struct mj_value mj_env_eval_expr(const struct mj_env *env,
                                 const char *expr,
                                 struct mj_value ctx);

/*
 Frees a MiniJinja environment.
 */
MINIJINJA_API void mj_env_free(struct mj_env *env);

/*
 Allocates a new and empty MiniJinja environment.
 */
MINIJINJA_API struct mj_env *mj_env_new(void);

/*
 Removes a template from the environment.
 */
MINIJINJA_API bool mj_env_remove_template(struct mj_env *env, const char *name);

/*
 Renders a template from a named string.
 */
MINIJINJA_API
char *mj_env_render_named_str(const struct mj_env *env,
                              const char *name,
                              const char *source,
                              struct mj_value ctx);

/*
 Renders a template registered on the environment.
 */
MINIJINJA_API
char *mj_env_render_template(const struct mj_env *env,
                             const char *name,
                             struct mj_value ctx);

/*
 Enables or disables debug mode.
 */
MINIJINJA_API void mj_env_set_debug(struct mj_env *env, bool val);

/*
 Preserve the trailing newline when rendering templates.
 */
MINIJINJA_API void mj_env_set_keep_trailing_newline(struct mj_env *env, bool val);

/*
 Enables or disables the `lstrip_blocks` feature.
 */
MINIJINJA_API void mj_env_set_lstrip_blocks(struct mj_env *env, bool val);

/*
 Changes the recursion limit.
 */
MINIJINJA_API void mj_env_set_recursion_limit(struct mj_env *env, uint32_t val);

/*
 Reconfigures the syntax.
 */
MINIJINJA_API
bool mj_env_set_syntax_config(struct mj_env *env,
                              const struct mj_syntax_config *syntax);

/*
 Enables or disables the `trim_blocks` feature.
 */
MINIJINJA_API void mj_env_set_trim_blocks(struct mj_env *env, bool val);

/*
 Reconfigures the undefined behavior.
 */
MINIJINJA_API
void mj_env_set_undefined_behavior(struct mj_env *env,
                                   enum mj_undefined_behavior val);

/*
 Clears the current error.
 */
MINIJINJA_API void mj_err_clear(void);

/*
 Returns the error's description if there is an error.
 */
MINIJINJA_API const char *mj_err_get_detail(void);

/*
 Returns the error's kind
 */
MINIJINJA_API enum mj_err_kind mj_err_get_kind(void);

/*
 Returns the error's current line.
 */
MINIJINJA_API uint32_t mj_err_get_line(void);

/*
 Returns the error's current template.
 */
MINIJINJA_API const char *mj_err_get_template_name(void);

/*
 Returns `true` if there is currently an error.
 */
MINIJINJA_API bool mj_err_is_set(void);

/*
 Prints the error to stderr.
 */
MINIJINJA_API bool mj_err_print(void);

/*
 Frees an engine allocated string.
 */
MINIJINJA_API void mj_str_free(char *s);

/*
 Sets the syntax to defaults.
 */
MINIJINJA_API void mj_syntax_config_default(struct mj_syntax_config *syntax);

/*
 Appends a value to a list
 */
MINIJINJA_API bool mj_value_append(struct mj_value *slf, struct mj_value value);

/*
 Extracts a float from the value
 */
MINIJINJA_API double mj_value_as_f64(struct mj_value value);

/*
 Extracts an integer from the value
 */
MINIJINJA_API int64_t mj_value_as_i64(struct mj_value value);

/*
 Extracts an unsigned integer from the value
 */
MINIJINJA_API uint64_t mj_value_as_u64(struct mj_value value);

/*
 Debug prints a value to stderr
 */
MINIJINJA_API void mj_value_dbg(struct mj_value value);

/*
 Decrements the refcount
 */
MINIJINJA_API void mj_value_decref(struct mj_value *value);

/*
 Looks up an element by an integer index in a list of object
 */
MINIJINJA_API struct mj_value mj_value_get_by_index(struct mj_value value, uint64_t idx);

/*
 Looks up an element by a string index in an object.
 */
MINIJINJA_API struct mj_value mj_value_get_by_str(struct mj_value value, const char *key);

/*
 Looks up an element by a value
 */
MINIJINJA_API struct mj_value mj_value_get_by_value(struct mj_value value, struct mj_value key);

/*
 Returns the value kind.
 */
MINIJINJA_API enum mj_value_kind mj_value_get_kind(struct mj_value value);

/*
 Increments the refcount
 */
MINIJINJA_API void mj_value_incref(struct mj_value *value);

/*
 Checks if the value is numeric
 */
MINIJINJA_API bool mj_value_is_number(struct mj_value value);

/*
 Checks if the value is truthy
 */
MINIJINJA_API bool mj_value_is_true(struct mj_value value);

/*
 Ends the iteration and deallocates the iterator
 */
MINIJINJA_API void mj_value_iter_free(struct mj_value_iter *iter);

/*
 Yields the next value from the iterator.
 */
MINIJINJA_API bool mj_value_iter_next(struct mj_value_iter *iter, struct mj_value *val_out);

/*
 Returns the length of the object
 */
MINIJINJA_API uint64_t mj_value_len(struct mj_value value);

/*
 Creates a new boolean value
 */
MINIJINJA_API struct mj_value mj_value_new_bool(bool value);

/*
 Creates a new f32 value
 */
MINIJINJA_API struct mj_value mj_value_new_f32(float value);

/*
 Creates a new f64 value
 */
MINIJINJA_API struct mj_value mj_value_new_f64(double value);

/*
 Creates a new i32 value
 */
MINIJINJA_API struct mj_value mj_value_new_i32(int32_t value);

/*
 Creates a new i64 value
 */
MINIJINJA_API struct mj_value mj_value_new_i64(int64_t value);

/*
 Creates an empty list value
 */
MINIJINJA_API struct mj_value mj_value_new_list(void);

/*
 Creates a new none value.
 */
MINIJINJA_API struct mj_value mj_value_new_none(void);

/*
 Creates an empty object value
 */
MINIJINJA_API struct mj_value mj_value_new_object(void);

/*
 Creates a new string value
 */
MINIJINJA_API struct mj_value mj_value_new_string(const char *s);

/*
 Creates a new u32 value
 */
MINIJINJA_API struct mj_value mj_value_new_u32(uint32_t value);

/*
 Creates a new u64 value
 */
MINIJINJA_API struct mj_value mj_value_new_u64(uint64_t value);

/*
 Creates a new undefined value.
 */
MINIJINJA_API struct mj_value mj_value_new_undefined(void);

/*
 Inserts a key into an object value.
 */
MINIJINJA_API
bool mj_value_set_key(struct mj_value *slf,
                      struct mj_value key,
                      struct mj_value value);

/*
 Inserts a string key into an object value.
 */
MINIJINJA_API
bool mj_value_set_string_key(struct mj_value *slf,
                             const char *key,
                             struct mj_value value);

/*
 Converts the value into a string.
 */
MINIJINJA_API char *mj_value_to_str(struct mj_value value);

/*
 Iterates over the value.
 */
MINIJINJA_API struct mj_value_iter *mj_value_try_iter(struct mj_value value);

#ifdef __cplusplus
} // extern "C"
#endif // __cplusplus

#endif /* _minijinja_h_included */
