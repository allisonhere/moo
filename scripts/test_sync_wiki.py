import os
import tempfile
import unittest
from pathlib import Path

from scripts import sync_wiki as sw

SLUGS = {"install", "configuration"}


class ConvertTests(unittest.TestCase):
    def convert(self, text):
        return sw.convert(text, "page", SLUGS)

    def test_docs_links_become_wiki_links_and_unknown_ones_point_at_source(self):
        out = self.convert("See [i](/docs/install/) and [c](/docs/configuration/#keys) and [x](/docs/other/).")
        self.assertIn("[i](Docs-install)", out)
        self.assertIn("[c](Docs-configuration#keys)", out)
        self.assertIn("[x](https://github.com/allisonhere/moo/blob/master/docs/next/website/src/content/docs/other.mdx)", out)

    def test_site_paths_are_made_absolute(self):
        self.assertIn("(https://github.com/allisonhere/moo/blob/master/docs/next/website/public/a.png)", self.convert("[s](/a.png)"))

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
        self.assertIn("Run `moo --default-config`", out)
        self.assertIn("[Configuration](Docs-configuration)", out)
        self.assertIn("allisonhere/moo/blob/master/", out)

    def test_placeholders_in_prose_are_not_treated_as_components(self):
        self.assertEqual(self.convert("Run `moo pane read <pane>`."), "Run `moo pane read <pane>`.")


class SourceIdentityTests(unittest.TestCase):
    def test_product_prose_and_compatibility_names_are_preserved(self):
        source = ('Moo uses `herdr-plugin.toml` and `min_herdr_version`.\n'
                  'Reports retain `herdr:codex`.\n\n'
                  '```toml\n[ui.toast.herdr]\nposition = "top_right"\n```\n'
                  '```sh\nmoo agent list\n```\n')
        self.assertEqual(sw.convert(source, "page", SLUGS), source)

    def test_source_is_authoritative_even_for_third_party_commands(self):
        source = 'Moo can coexist with Herdr.\n```sh\nherdr --version\n```\n'
        self.assertEqual(sw.convert(source, "page", SLUGS), source)


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


@unittest.skipIf(os.name == "nt", "the fake moo is a POSIX shell script")
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
        (self.docs / "install.mdx").write_text("---\ntitle: Install Moo\ndescription: How.\n---\nHi\n")
        (self.docs / "moo.mdx").write_text("---\ntitle: Moo\n---\nOurs\n")
        (self.docs / "index.mdx").write_text("---\ntitle: Home\n---\nlanding\n")

    def test_pages_lead_with_moo_and_credit_changes_in_a_footer(self):
        (self.docs / "install.mdx").write_text(
            "---\ntitle: Install Moo\ndescription: How.\n---\n"
            "Install Moo.\n\n[Credits and documentation changes](/docs/credits/).\n"
        )
        (self.docs / "credits.mdx").write_text(
            "---\ntitle: Credits\n---\nMoo builds on Herdr under Apache-2.0.\n"
        )
        sw.sync(self.docs, self.wiki)
        page = (self.wiki / "Docs-install.md").read_text()
        self.assertIn(sw.MARKER, page)
        self.assertIn("# Install Moo", page)
        self.assertIn("[Credits and documentation changes](Docs-credits)", page)
        self.assertNotIn("Adapted from", page)
        self.assertNotIn("upstream Herdr", page)
        self.assertIn("Moo builds on Herdr under Apache-2.0", (self.wiki / "Docs-credits.md").read_text())
        self.assertTrue((self.wiki / "Fork-guide.md").exists())
        self.assertFalse((self.wiki / "Docs-index.md").exists())
        self.assertIn("[Install Moo](Docs-install)", (self.wiki / "Documentation.md").read_text())

    def test_a_second_run_changes_nothing_and_check_reports_pending_work(self):
        self.assertTrue(sw.sync(self.docs, self.wiki, check=True))
        sw.sync(self.docs, self.wiki)
        self.assertEqual(sw.sync(self.docs, self.wiki), [])
        (self.docs / "install.mdx").write_text("---\ntitle: Install Moo\n---\nChanged\n")
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
        self.assertIn("[Install Moo](Docs-install)", sidebar)
        self.assertNotIn("(Home)](Home)", sidebar.split("**Documentation**")[0].split("\n", 3)[3])


if __name__ == "__main__":
    unittest.main()
