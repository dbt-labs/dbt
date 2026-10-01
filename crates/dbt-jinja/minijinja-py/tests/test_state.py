from minijinja import Environment, safe, pass_state


def test_func_state():
    env = Environment()

    @pass_state
    def my_func(state):
        assert state.name == "template-name"
        assert state.auto_escape is None
        assert state.current_block == "foo"
        assert state.lookup("bar") == 23
        assert state.lookup("aha") is None
        assert state.lookup("my_func") is my_func
        assert state.env is env
        return 42

    rv = env.render_str(
        "{% block foo %}{{ my_func() }}{% endblock %}",
        "template-name",
        my_func=my_func,
        bar=23,
    )
    assert rv == "42"


def test_global_func_state():
    env = Environment()

    @pass_state
    def my_func(state):
        assert state.name == "template-name"
        assert state.auto_escape is None
        assert state.current_block == "foo"
        assert state.lookup("bar") == 23
        assert state.lookup("aha") is None
        assert state.env is env
        return 42

    env.add_global("my_func", my_func)

    rv = env.render_str(
        "{% block foo %}{{ my_func() }}{% endblock %}",
        "template-name",
        bar=23,
    )
    assert rv == "42"


def test_filter_state():
    env = Environment()

    @pass_state
    def my_filter(state, value):
        assert state.name == "template-name"
        assert state.auto_escape is None
        assert state.current_block == "foo"
        assert state.lookup("bar") == 23
        assert state.lookup("aha") is None
        assert state.env is env
        return value

    env.add_filter("myfilter", my_filter)

    rv = env.render_str(
        "{% block foo %}{{ 42|myfilter }}{% endblock %}",
        "template-name",
        bar=23,
    )
    assert rv == "42"


def test_test_state():
    env = Environment()

    @pass_state
    def my_test(state, value):
        assert state.name == "template-name"
        assert state.auto_escape is None
        assert state.current_block == "foo"
        assert state.lookup("bar") == 23
        assert state.lookup("aha") is None
        assert state.env is env
        return True

    env.add_test("mytest", my_test)

    rv = env.render_str(
        "{% block foo %}{{ 42 is mytest }}{% endblock %}",
        "template-name",
        bar=23,
    )
    assert rv == "true"
