import tempfile
import unittest
from pathlib import Path

from scripts import sync_wiki as sw

SLUGS = {"install", "configuration"}


class ConvertTests(unittest.TestCase):
    def convert(self, text):
        return sw.convert(text, "page", SLUGS)

    def test_docs_links_become_wiki_links_and_unknown_ones_point_at_the_site(self):
        out = self.convert("See [i](/docs/install/) and [c](/docs/configuration/#keys) and [x](/docs/other/).")
        self.assertIn("[i](Docs-install)", out)
        self.assertIn("[c](Docs-configuration#keys)", out)
        self.assertIn("[x](https://herdr.dev/docs/other/)", out)

    def test_site_paths_are_made_absolute(self):
        self.assertIn("(https://herdr.dev/install.sh)", self.convert("[s](/install.sh)"))

    def test_code_fences_are_left_exactly_as_written(self):
        text = "```bash\nimport x\n[a](/docs/install/)\n:::note\n```"
        self.assertEqual(self.convert(text), text)

    def test_asides_become_blockquotes(self):
        out = self.convert(":::note\nBe careful.\n\nSecond.\n:::\nafter")
        self.assertEqual(out, "> **Note**\n> Be careful.\n>\n> Second.\nafter")
        self.assertIn("> **Heads up**", self.convert(":::tip[Heads up]\nx\n:::"))

    def test_imports_components_and_screenshot_blocks_are_dropped_or_replaced(self):
        out = self.convert(
            "import X from './x.astro';\n\n<ConfigReference />\n\n"
            '<div class="shots">\n  <figure><img src="/a.png" alt="a" /></figure>\n</div>\n'
        )
        self.assertNotIn("import", out)
        self.assertNotIn("<ConfigReference", out)
        self.assertNotIn("<img", out)
        self.assertIn("herdr.dev/docs/page/", out)

    def test_placeholders_in_prose_are_not_treated_as_components(self):
        self.assertEqual(self.convert("Run `herdr pane read <pane>`."), "Run `moo pane read <pane>`.")


class RewriteTests(unittest.TestCase):
    def test_a_command_that_starts_with_herdr_becomes_moo(self):
        for before, after in [
            ("herdr agent list", "moo agent list"),
            ("herdr", "moo"),
            ("$ herdr update", "$ moo update"),
            ("split=$(herdr pane split --current)", "split=$(moo pane split --current)"),
            ("herdr a && herdr b | herdr c", "moo a && moo b | moo c"),
            ("herdr --remote box", "moo --remote box"),
        ]:
            self.assertEqual(sw.rewrite_code(before), after, before)

    def test_package_names_urls_and_manifest_names_stay_as_upstream_wrote_them(self):
        for unchanged in [
            "brew install herdr",
            "mise use -g herdr",
            "npx skills add herdrdev/herdr --skill herdr -g",
            "curl -fsSL https://herdr.dev/install.sh | sh",
            "herdr-plugin.toml",
            "ui.toast.herdr.position",
            "herdr not found in mise tool registry",
        ]:
            self.assertEqual(sw.rewrite_code(unchanged), unchanged, unchanged)

    def test_paths_use_moo_names_but_not_inside_urls(self):
        self.assertEqual(sw.rewrite_code("~/.config/herdr/config.toml"), "~/.config/moo/config.toml")
        self.assertEqual(sw.rewrite_code("~/.local/state/herdr/x"), "~/.local/state/moo/x")
        self.assertEqual(sw.rewrite_code("herdr.sock and herdr-client.sock"), "moo.sock and moo-client.sock")
        self.assertEqual(sw.rewrite_code(r"%APPDATA%\herdr\config.toml"), r"%APPDATA%\moo\config.toml")
        self.assertEqual(sw.rewrite_code("C:\\bin\\herdr.exe"), "C:\\bin\\moo.exe")
        self.assertEqual(sw.rewrite_code("see https://example.com/.config/herdr/x"), "see https://example.com/.config/herdr/x")

    def test_only_code_is_rewritten_never_prose(self):
        text = "Herdr runs `herdr status` and herdr stays herdr.\n\n```sh\nherdr agent list\n```\n"
        self.assertEqual(
            sw.convert(text, "page", SLUGS),
            "Herdr runs `moo status` and herdr stays herdr.\n\n```sh\nmoo agent list\n```\n",
        )

    def test_upstream_only_commands_keep_their_name(self):
        sw.UPSTREAM_ONLY.add(("legacy", "thing"))
        self.addCleanup(sw.UPSTREAM_ONLY.discard, ("legacy", "thing"))
        self.assertEqual(sw.rewrite_code("herdr legacy thing --x"), "herdr legacy thing --x")
        self.assertEqual(sw.rewrite_code("herdr legacy other"), "moo legacy other")


FAKE_MOO = """#!/bin/sh
# a stand-in CLI: `agent` has subcommands (list), `status` is a leaf; anything else shows the general help
args=$(printf '%s ' "$@" | sed 's/ *--help *$//')
case "$args" in
  "agent") printf 'Usage: moo agent\nCommands:\n  list\n' ;;
  "agent list") printf 'Usage: moo agent list\n' ;;
  "status") printf 'Usage: moo status [server]\n' ;;
  "agent "*|"status "*) printf 'error: unexpected argument\n' >&2; exit 2 ;;
  *) printf 'Usage: moo [options]\nCommands:\n  agent\n' ;;
esac
"""


class VerifyTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.binary = self.root / "moo"
        self.binary.write_text(FAKE_MOO)
        self.binary.chmod(0o755)
        self.wiki = self.root / "wiki"
        self.wiki.mkdir()

    def verify(self, page):
        (self.wiki / "Docs-x.md").write_text(page)
        return sw.verify(str(self.binary), self.wiki)

    def test_commands_that_exist_pass_and_the_words_after_a_leaf_are_arguments(self):
        page = "```sh\nmoo agent list\nmoo status server --json\nmoo status whatever\nmoo --remote box\n```\nUse `moo agent list`.\n"
        self.assertEqual(self.verify(page), [])

    def test_a_missing_subcommand_or_command_is_reported(self):
        page = "```sh\nmoo agent nope\nmoo bogus\n```\n"
        self.assertEqual(self.verify(page), ["moo agent nope", "moo bogus"])

    def test_moo_commands_reads_fences_and_inline_code_but_not_prose(self):
        text = "moo outside prose\n```sh\nmoo agent list --x\n```\nand `moo status`\n"
        self.assertEqual(sw.moo_commands(text), {("agent", "list"), ("status",)})


class FrontmatterTests(unittest.TestCase):
    def test_title_and_description_are_read_and_removed(self):
        meta, body = sw.split_frontmatter('---\ntitle: "Hello"\ndescription: A page.\nhero:\n  x: 1\n---\nBody\n')
        self.assertEqual(meta["title"], "Hello")
        self.assertEqual(meta["description"], "A page.")
        self.assertNotIn("hero", meta)
        self.assertEqual(body, "Body\n")


class SyncTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        root = Path(self.tmp.name)
        self.docs, self.wiki = root / "docs", root / "wiki"
        self.docs.mkdir()
        self.wiki.mkdir()
        (self.docs / "install.mdx").write_text("---\ntitle: Install Herdr\ndescription: How.\n---\nHi\n")
        (self.docs / "moo.mdx").write_text("---\ntitle: Moo\n---\nOurs\n")
        (self.docs / "index.mdx").write_text("---\ntitle: Home\n---\nlanding\n")

    def test_pages_are_generated_with_attribution_only_for_upstream_ones(self):
        sw.sync(self.docs, self.wiki)
        upstream = (self.wiki / "Docs-install.md").read_text()
        self.assertIn(sw.MARKER, upstream)
        self.assertIn("Adapted from [Herdr's documentation]", upstream)
        self.assertIn("Apache-2.0", upstream)
        self.assertNotIn("Adapted from", (self.wiki / "Fork-guide.md").read_text())
        self.assertFalse((self.wiki / "Docs-index.md").exists())
        self.assertIn("[Install Herdr](Docs-install)", (self.wiki / "Documentation.md").read_text())

    def test_a_second_run_changes_nothing_and_check_reports_pending_work(self):
        self.assertTrue(sw.sync(self.docs, self.wiki, check=True))
        sw.sync(self.docs, self.wiki)
        self.assertEqual(sw.sync(self.docs, self.wiki), [])
        (self.docs / "install.mdx").write_text("---\ntitle: Install Herdr\n---\nChanged\n")
        self.assertEqual(sw.sync(self.docs, self.wiki, check=True), ["Docs-install.md", "Documentation.md"])

    def test_a_hand_written_page_is_never_overwritten(self):
        (self.wiki / "Docs-install.md").write_text("mine")
        with self.assertRaises(SystemExit):
            sw.sync(self.docs, self.wiki)
        self.assertEqual((self.wiki / "Docs-install.md").read_text(), "mine")

    def test_removed_docs_remove_their_generated_page_but_not_hand_written_ones(self):
        (self.wiki / "Agent-handoff.md").write_text("hand written")
        sw.sync(self.docs, self.wiki)
        (self.docs / "install.mdx").unlink()
        sw.sync(self.docs, self.wiki)
        self.assertFalse((self.wiki / "Docs-install.md").exists())
        self.assertTrue((self.wiki / "Agent-handoff.md").exists())

    def test_sidebar_lists_hand_written_pages_apart_from_the_mirror(self):
        (self.wiki / "Agent-handoff.md").write_text("hand written")
        (self.wiki / "Home.md").write_text("home")
        sw.sync(self.docs, self.wiki)
        sidebar = (self.wiki / "_Sidebar.md").read_text()
        self.assertIn("[Agent handoff](Agent-handoff)", sidebar)
        self.assertIn("[Install Herdr](Docs-install)", sidebar)
        self.assertNotIn("(Home)](Home)", sidebar.split("**Documentation**")[0].split("\n", 3)[3])


if __name__ == "__main__":
    unittest.main()
