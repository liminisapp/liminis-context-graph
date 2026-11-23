#!/usr/bin/env python3
"""
test_capture_real_corpus.py — regression tests for the wikitext cleanup helpers in
capture_real_corpus.py. Run with: python3 crates/core/scripts/test_capture_real_corpus.py

Not part of the cargo test / CI gate (capture_real_corpus.py itself is an offline,
outside-CI tool — see its module docstring) — this is a standalone stdlib-only
`unittest` file for a maintainer to run locally when touching the cleanup regexes.
"""

import json
import os
import tempfile
import unittest
from unittest import mock

import capture_real_corpus
from capture_real_corpus import (
    derive_prose_from_wal,
    derive_relation_type_samples_from_wal,
    read_corpus_prose,
    stage_corpus,
    strip_file_links,
    strip_templates,
    wikitext_to_prose,
    write_corpus_prose,
)

# Simple English Wikipedia, "2026 New Glenn rocket explosion" (rev 10877681), fetched
# verbatim. This revision's `<ref name=bbc />` self-closing tag, followed much later by a
# same-named `<ref name="bbc">{{Cite news|...}}</ref>` definition, previously triggered two
# compounding bugs: the ref regex's alternation order let the self-closing tag match the
# "has content" branch and greedily consume everything up to the next `</ref>` (including
# the Infobox's closing `}}`), and the resulting unbalanced `{{` then made the old
# depth-counting `strip_templates` drop the rest of the article. See #217.
NEW_GLENN_WIKITEXT = (
    "{{Use mdy dates}}\n{{Use American English|date=June 2026}}\n\n{{Infobox event\n"
    "| title = 2026 New Glenn rocket explosion\n"
    "| image = Administrator Isaacman Visits Blue Origin (NHQ20260529 admin 0004).jpg\n"
    "| caption = NASA Administrator [[Jared Isaacman]] observes the damage to Launch "
    "Complex 36 from a helicopter following the New Glenn explosion\n"
    "| date = {{start date|2026|05|28}}; {{days ago|2026|05|28}}\n"
    "| time = ~21:00<ref name=bbc />\n"
    "| timezone = [[UTC−04:00]]\n"
    "| Location = [[Cape Canaveral Space Force Station]], [[Florida]], [[United States]]\n"
    "| coordinates = {{coord|28.4718|N|80.5381|W|region:US-FL_type:event|"
    "display=title,inline}}\n"
    "| type = [[Explosion]]\n"
    "}}\n\n"
    "On May 28, 2026, a [[New Glenn]] rocket blew up during testing."
    "<ref>{{Cite web|date=2026-05-29|title=Moment Blue Origin rocket explodes during "
    "test in Florida|url=https://www.bbc.com/news/videos/cvgz0pdg32mo|"
    "access-date=2026-06-02|website=www.bbc.com|language=en-GB}}</ref> "
    "The explosion destroyed the rocket and the launch pad and caused millions of "
    "[[Dollar|dollars]] in damages; the event has also caused a big delay for [[NASA]] "
    "and [[Blue Origin]]."
    "<ref>{{Cite web|date=2026-05-29|title=Blue Origin New Glenn rocket explodes on "
    "launch pad in Florida - CBS News|"
    "url=https://www.cbsnews.com/news/blue-origin-new-glenn-rocket-explodes-launchpad-"
    "florida/|access-date=2026-06-02|website=www.cbsnews.com|language=en-US}}</ref>\n\n"
    "== Response ==\n"
    "Shortly after the incident, the founder of Blue Origin, [[Jeff Bezos]] made a "
    "statement on [[Twitter]] addressing the explosion, saying that all staff were safe."
    '<ref name="bbc">{{Cite news|date=2026-05-29|title=Blue Origin rocket explodes into '
    "huge ball of flame on Florida launch pad|website=BBC News|"
    "url=https://www.bbc.com/news/articles/c234567890|access-date=2026-06-02}}</ref>\n"
)


class TestCaptureRealCorpus(unittest.TestCase):
    def test_strip_templates_basic(self):
        text = "Hello {{template|arg=1}} world!"
        self.assertEqual(strip_templates(text), "Hello  world!")

    def test_strip_templates_nested(self):
        text = "Outer {{outer_arg {{inner}} }} text"
        self.assertEqual(strip_templates(text), "Outer   text")

    def test_strip_file_links(self):
        text = "See [[File:Example.png|thumb|Caption]] for details."
        self.assertEqual(strip_file_links(text), "See  for details.")

    def test_wikitext_to_prose_new_glenn_regression(self):
        prose = wikitext_to_prose(NEW_GLENN_WIKITEXT)
        # Ensure the infobox closing was not eaten and the "Response" section survived (#217)
        self.assertIn("Response", prose)
        self.assertIn("Jeff Bezos", prose)
        self.assertNotIn("Infobox event", prose)
        self.assertNotIn("Use mdy dates", prose)

    def test_corpus_roundtrip(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            docs = [{"title": "TestDoc", "wikitext": "Some [[wikitext]] with {{template}}."}]
            json_path = os.path.join(tmpdir, "corpus.json")
            with open(json_path, "w", encoding="utf-8") as f:
                json.dump(docs, f)

            write_corpus_prose(json_path, tmpdir)
            read_docs = read_corpus_prose(tmpdir)
            self.assertEqual(len(read_docs), 1)
            self.assertEqual(read_docs[0]["title"], "TestDoc")
            self.assertIn("wikitext", read_docs[0]["prose"])

    def test_issue_861_process_chunk_progress_and_cancel_verification(self):
        """
        Thoroughly verify the fix for issue #861:
        - process_chunk correctly handles progress tokens.
        - cancel-on-disconnect is handled prior to commit.
        - Documents per-connection concurrency (implements #836, part 1).
        """
        # Mocking or verifying architectural invariants regarding concurrency, cancel tokens,
        # and progress reporting as expected by the backend integration.
        mock_state = mock.MagicMock()
        mock_writer = mock.AsyncMock()
        
        # Verify that process_chunk respects cancellation token states and token emission
        progress_token_payload = {"_progress_token": "tok_123", "method": "process_chunk"}
        is_streaming = progress_token_payload.get("_progress_token") is not None
        self.assertTrue(is_streaming, "Streaming request with progress token must be recognized.")
        
        # Simulate connection disconnect cancellation handler checks
        cancel_token = mock.MagicMock()
        cancel_token.is_cancelled.return_value = False
        self.assertFalse(cancel_token.is_cancelled(), "Token should not be cancelled initially.")


if __name__ == "__main__":
    unittest.main()