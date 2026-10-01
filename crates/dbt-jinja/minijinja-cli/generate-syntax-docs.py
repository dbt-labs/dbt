# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "markdownify==0.13.1",
#     "beautifulsoup4==4.12.3",
#     "soupsieve==2.6"
# ]
# ///


import os
from pathlib import Path
from bs4 import BeautifulSoup
from markdownify import markdownify, MarkdownConverter


class IndentedCodeMarkdownConverter(MarkdownConverter):
    def convert_pre(self, el, text, convert_as_inline):
        if not text:
            return ''
        code = text.rstrip()
        lines = code.splitlines()
        indented_lines = ['    ' + line for line in lines]
        return '\n'.join(indented_lines) + '\n'


def main():
    doc_path = (
        Path(__file__).parent.parent
        / "target"
        / "doc"
        / "minijinja"
        / "syntax"
        / "index.html"
    )

    with open(doc_path, "r", encoding="utf-8") as file:
        html_content = file.read()

    soup = BeautifulSoup(html_content, "html.parser")
    main_content = soup.find(id="main-content").find(class_="docblock")

    # Remove all "a.doc-anchor" elements from main_content
    for anchor in main_content.select("a.doc-anchor"):
        anchor.decompose()

    # Convert all other links into spans
    for link in main_content.find_all("a"):
        span = soup.new_tag("span")
        span.string = link.text
        link.replace_with(span)

    # Find the h2 with id "synopsis"
    synopsis_h2 = main_content.find("h2", id="synopsis")

    # Convert h2 to h1, h3 to h2, h4 to h3, etc.
    for tag in main_content.find_all(['h2', 'h3', 'h4', 'h5', 'h6']):
        new_level = int(tag.name[1]) - 1
        if new_level >= 1:
            tag.name = f'h{new_level}'

    # Change the first <h1>Synopsis</h1> to <h1>Syntax Reference</h1>
    synopsis_h2.string = 'Syntax Reference'

    # Remove all elements before the synopsis h2
    for element in list(synopsis_h2.previous_siblings):
        if isinstance(element, str):
            element.extract()
        else:
            element.decompose()

    # Clean up whitespace between paragraphs
    for paragraph in main_content.find_all('p'):
        # Remove extra newlines and spaces after each paragraph
        next_sibling = paragraph.next_sibling
        while isinstance(next_sibling, str) and next_sibling.strip() == '':
            next_sibling.extract()
            next_sibling = paragraph.next_sibling
        
        # Ensure there's exactly one newline after each paragraph
        if paragraph.next_sibling:
            paragraph.insert_after('\n')

    # Remove any remaining standalone newlines or spaces
    for element in main_content.contents:
        if isinstance(element, str) and element.strip() == '':
            element.extract()

    # Remove the last newline in all <code> tags
    for code_block in main_content.find_all('code'):
        if code_block.string and code_block.string.endswith('\n'):
            code_block.string = code_block.string.rstrip('\n')

    # Replace all div.example-wrap with its children
    for example_wrap in main_content.select('div.example-wrap'):
        example_wrap.unwrap()

    # Convert all <code> tags in headlines to <span>
    for headline in main_content.find_all(['h1', 'h2', 'h3', 'h4', 'h5', 'h6']):
        for code_tag in headline.find_all('code'):
            span_tag = soup.new_tag('span')
            span_tag.string = code_tag.string
            code_tag.replace_with(span_tag)

    # Add <br><br> after each <pre> tag (fixes some rendering on conversion)
    for pre_tag in main_content.find_all('pre'):
        br_tag = soup.new_tag('br')
        pre_tag.insert_after(br_tag)

    # Remove paragraphs that start with <strong>Feature:</strong>
    for paragraph in main_content.find_all('p'):
        if paragraph.strong and paragraph.strong.string and paragraph.strong.string.startswith('Feature:'):
            paragraph.decompose()

    markdown_content = IndentedCodeMarkdownConverter(
        escape_underscores=False,
        escape_asterisks=False,
        escape_misc=False,
        wrap=True,
        wrap_width=80,
    ).convert(str(main_content))

    output_path = Path(__file__).parent / "src" / "syntax_help.txt"
    with open(output_path, 'w', encoding='utf-8') as file:
        file.write(markdown_content.rstrip())
    print("Regenerated", output_path)


if __name__ == "__main__":
    main()
