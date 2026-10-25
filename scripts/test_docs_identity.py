"""Check the current Moo docs without rewriting historical or third-party records."""

import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
DOCS = ROOT / "docs/next/website/src/content/docs"


class DocumentationIdentityTests(unittest.TestCase):
    def test_current_guides_never_install_the_upstream_product(self):
        paths = list(DOCS.rglob("*.mdx")) + [
            ROOT / "README.md",
            ROOT / "README.zh-CN.md",
            ROOT / "docs/next/README.md",
            ROOT / "docs/next/README.zh-CN.md",
            ROOT / "distribution/agent-guide.md",
            ROOT / "skills/moo/SKILL.md",
        ]
        forbidden = re.compile(
            r"https://herdr\.dev/(?:install\.(?:sh|ps1|cmd)|latest\.json|preview\.json)"
            r"|brew install herdr|mise use -g herdr|nix (?:run|build|profile install) github:herdrdev/herdr"
            r"|npx skills add herdrdev/herdr"
        )
        for path in paths:
            with self.subTest(path=path.relative_to(ROOT)):
                self.assertIsNone(forbidden.search(path.read_text()))

    def test_page_metadata_leads_with_moo(self):
        for path in DOCS.rglob("*.mdx"):
            with self.subTest(path=path.relative_to(DOCS)):
                metadata = path.read_text().split("---", 2)[1]
                self.assertNotRegex(metadata, r"[Hh]erdr|temporary name|development fork")

    def test_local_document_links_have_a_source_page(self):
        for path in DOCS.rglob("*.mdx"):
            for slug in re.findall(r"\]\(/(?:ja/|zh-cn/)?docs/([a-z0-9-]+)/?", path.read_text()):
                with self.subTest(path=path.relative_to(DOCS), target=slug):
                    self.assertTrue((DOCS / f"{slug}.mdx").is_file())

    def test_configuration_examples_keep_real_compatibility_names(self):
        for locale in ("", "ja", "zh-cn"):
            with self.subTest(locale=locale):
                config = (DOCS / locale / "configuration.mdx").read_text()
                api = (DOCS / locale / "socket-api.mdx").read_text()
                self.assertIn('[ui.toast.herdr]', config)
                self.assertIn('delivery = "herdr"', config)
                self.assertNotIn('ui.toast.moo', config + api)
                self.assertNotIn('delivery = "moo"', config + api)
                self.assertIn('herdr-plugin.toml', api)
                self.assertIn('min_herdr_version', api)
                self.assertIn('herdr:codex', api)

    def test_readme_and_staged_readme_agree(self):
        for name in ("README.md", "README.zh-CN.md"):
            with self.subTest(name=name):
                self.assertEqual((ROOT / name).read_text(), (ROOT / "docs/next" / name).read_text())


if __name__ == "__main__":
    unittest.main()
