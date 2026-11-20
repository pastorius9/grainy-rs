"""Offline regression checks for publication gates."""
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

import layout
import make_public
import notices
import scan_public


class PublicationTests(unittest.TestCase):
    def test_review_inputs_are_removed(self):
        for name in ('NEON_REVIEW.md', 'REVIEW2.md', 'nested/worker_BRIEF.md', 'CODEX_FINAL.md'):
            self.assertTrue(any(make_public.matches(name, rule) for rule in make_public.REMOVE))

    def test_missing_names_and_flagged_licences_stop_publication(self):
        with TemporaryDirectory() as root:
            self.assertEqual(scan_public.main([root, '--names', str(Path(root) / 'absent')]), 2)
        with self.assertRaises(SystemExit):
            notices.require_reviewed([('generated crate', 'unknown')])

    def test_only_the_documented_font_package_exception_is_accepted(self):
        expression = '(MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0'
        notices.require_reviewed([('epaint_default_fonts 0.37.0', expression)])
        for item in [('epaint_default_fonts 0.37.1', expression),
                     ('epaint_default_fonts 0.37.0', 'no licence file found in the crate package')]:
            with self.assertRaises(SystemExit):
                notices.require_reviewed([item])

    def test_release_artifacts_require_success(self):
        workflow = (make_public.REPO / '.github/workflows/build.yml').read_text(encoding='utf-8')
        self.assertNotIn('!cancelled()', workflow)
        self.assertIn('Clippy (Apple Silicon)', workflow)

    def test_binary_path_check_handles_foreign_slashes_case_and_utf16(self):
        with TemporaryDirectory() as tmp:
            binary = Path(tmp) / 'program'
            root = Path('C:/private/generated project')
            for text in ('C:/Private/Generated Project', r'C:\Private\Generated Project'):
                for encoding in ('utf-8', 'utf-16le'):
                    binary.write_bytes(text.encode(encoding))
                    with self.assertRaises(SystemExit):
                        layout.check_binary_paths(binary, [root])
            binary.write_bytes(b'/build/grainy/src/app.rs')
            layout.check_binary_paths(binary, [root])
        flags = layout.release_env(True)['CARGO_ENCODED_RUSTFLAGS'].split('\x1f')
        self.assertIn('target-feature=+crt-static', flags)
        self.assertIn(f'--remap-path-prefix={layout.REPO}=/build/grainy', flags)
