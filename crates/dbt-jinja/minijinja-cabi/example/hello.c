#include <minijinja.h>
#include <stdio.h>
#include <assert.h>

int main()
{
    mj_env *env = mj_env_new();
    mj_env_set_debug(env, true);

    bool ok = mj_env_add_template(env, "hello", "\
Hello {{ name }}!\n\
{%- for item in seq %}\n\
  - {{ item }}\n\
{%- endfor %}\n\
seq: {{ seq }}");

    if (!ok) {
        mj_err_print();
        return 1;
    }
    assert(ok);

    // use objects for contexts
    mj_value ctx = mj_value_new_object();

    // shows how a list value is being created
    mj_value seq = mj_value_new_list();
    mj_value_append(&seq, mj_value_new_string("First"));
    mj_value_append(&seq, mj_value_new_string("Second"));
    mj_value_append(&seq, mj_value_new_i64(42));

    // values can be iterated over
    mj_value_iter *iter = mj_value_try_iter(seq);
    mj_value v;
    int idx = 0;
    while (mj_value_iter_next(iter, &v)) {
        char *vs = mj_value_to_str(v);
        fprintf(stderr, "value %d: %s\n", ++idx, vs);
        mj_str_free(vs);
    }
    mj_value_iter_free(iter);

    // store the values in the struct
    mj_value_set_string_key(&ctx, "seq", seq);
    mj_value_set_string_key(&ctx, "name", mj_value_new_string("C-Lang"));

    // render a template
    char *rv = mj_env_render_template(env, "hello", ctx);
    if (!rv) {
        mj_err_print();
    } else {
        printf("%s\n", rv);
        mj_str_free(rv);
    }

    // eval an expression
    mj_value erv = mj_env_eval_expr(env, "1 + 2", mj_value_new_object());
    char *ervs = mj_value_to_str(erv);
    fprintf(stderr, "1 + 2 = %s\n", ervs);
    mj_str_free(ervs);
    mj_value_decref(&erv);

    mj_env_free(env);

    return 0;
}
