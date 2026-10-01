def test_docs_generate_writes_embedded_site(tmp_project, invoke):
    project = tmp_project("hello_world")

    result = invoke(project, "docs", "generate")

    assert result.success, result.exception
    assert (project / "target" / "index.html").is_file()
