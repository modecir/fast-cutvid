"""Check release policy against real, isolated Git histories."""

import pathlib
import subprocess
import sys
import tempfile
import unittest


VERIFY = pathlib.Path(__file__).resolve().parents[1] / "packaging/verify_release_ref.py"


class ReleaseRefTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.git("init", "-b", "main")
        self.git("config", "user.name", "Release test")
        self.git("config", "user.email", "release-test@example.invalid")
        self.git("commit", "--allow-empty", "-m", "Initial release")
        self.git("tag", "v1.0.0")
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")

    def git(self, *arguments):
        return subprocess.check_output(
            ["git", *arguments], cwd=self.directory.name, stderr=subprocess.STDOUT, text=True
        ).strip()

    def verify(self, tag):
        return subprocess.run(
            [sys.executable, str(VERIFY), tag], cwd=self.directory.name,
            capture_output=True, text=True,
        )

    def test_existing_final_tag_is_accepted(self):
        self.assertEqual(self.verify("v1.0.0").returncode, 0)

    def test_unmerged_development_tag_is_rejected(self):
        self.git("switch", "-c", "dev")
        self.git("commit", "--allow-empty", "-m", "Development")
        self.git("tag", "v1.1.0")
        self.assertNotEqual(self.verify("v1.1.0").returncode, 0)

    def test_tag_must_point_to_main_merge_not_feature_parent(self):
        self.git("switch", "-c", "codex/release/1.1.0")
        self.git("commit", "--allow-empty", "-m", "Prepare release")
        self.git("tag", "v1.1.0")
        self.git("switch", "main")
        self.git("merge", "--no-ff", "codex/release/1.1.0", "-m", "Final release")
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")
        self.assertNotEqual(self.verify("v1.1.0").returncode, 0)
        self.git("tag", "-a", "v1.1.1", "-m", "Final annotated tag")
        self.assertEqual(self.verify("v1.1.1").returncode, 0)
        self.assertEqual(self.verify("v1.0.0").returncode, 0)

    def test_prerelease_malformed_and_missing_tags_are_rejected(self):
        for tag in ["v1.1.0-rc.1", "v1.1.0+build", "v01.1.0", "1.1.0", "v9.9.9", "--help"]:
            with self.subTest(tag=tag):
                self.assertNotEqual(self.verify(tag).returncode, 0)


if __name__ == "__main__":
    unittest.main()
