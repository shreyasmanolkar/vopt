#!/usr/bin/env python3
"""Real FFmpeg integration checks. Run after cargo build: python3 tests/smoke.py."""
import hashlib
import json
import os
import pathlib
import signal
import subprocess
import tempfile
import time
import unittest

BIN = pathlib.Path(__file__).resolve().parents[1] / 'target/debug/vopt'


def tool(*args):
    return subprocess.run(args, capture_output=True, check=True)


class Integration(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='vopt-tests-')
        cls.root = pathlib.Path(cls.tmp.name)
        cls.source = cls.root / 'source space café.mkv'
        tool('ffmpeg', '-v', 'error', '-f', 'lavfi', '-i', 'testsrc2=size=320x240:rate=24',
             '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=48000', '-t', '2',
             '-c:v', 'ffv1', '-c:a', 'flac', str(cls.source))
        cls.digest = hashlib.sha256(cls.source.read_bytes()).hexdigest()

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def run_cli(self, *args, code=0):
        p = subprocess.run([str(BIN), *map(str, args), '--json'], capture_output=True, text=True)
        self.assertEqual(p.returncode, code, p.stderr + p.stdout)
        return json.loads(p.stdout)

    def tearDown(self):
        self.assertEqual(hashlib.sha256(self.source.read_bytes()).hexdigest(), self.digest)
        self.assertFalse(list(self.root.glob('.vopt-*')))

    def test_doctor_and_inspect(self):
        self.assertTrue(self.run_cli('doctor')['encoders']['libsvtav1'])
        self.assertEqual(len(self.run_cli('inspect', self.source)['media']['streams']), 2)

    def test_dry_run(self):
        out = self.root / 'dry.mkv'
        p = self.run_cli(self.source, '-o', out, '--dry-run')
        self.assertEqual(p['status'], 'planned')
        self.assertFalse(out.exists())

    def test_av1_encoding(self):
        out = self.root / 'modern.mkv'
        p = self.run_cli(self.source, '-o', out, '--effort', 'quick')
        self.assertEqual(p['status'], 'optimized')
        self.assertLess(out.stat().st_size, self.source.stat().st_size)
        self.assertEqual(p['quality_verification'], 'unmeasured_presets')
        self.run_cli(self.source, '-o', out, code=2)

    def test_compatible_encoding(self):
        source = self.root / 'compatible-source.mp4'
        tool('ffmpeg', '-v', 'error', '-i', str(self.source), '-c:v', 'libx264',
             '-crf', '0', '-c:a', 'aac', str(source))
        out = self.root / 'compatible.mp4'
        p = self.run_cli(source, '--profile', 'compatible', '-o', out)
        self.assertEqual(p['status'], 'optimized')

    def test_hevc_encoding(self):
        if not self.run_cli('doctor')['encoders']['libx265']:
            self.skipTest('FFmpeg lacks libx265')
        source = self.root / 'hevc-source.mkv'
        tool('ffmpeg', '-v', 'error', '-i', str(self.source), '-map', '0:v', '-c:v', 'copy', str(source))
        out = self.root / 'hevc-output.mp4'
        result = self.run_cli(source, '--profile', 'hevc', '--effort', 'quick', '-o', out)
        self.assertEqual(result['encoder'], 'libx265')
        self.assertEqual(self.run_cli('inspect', out)['media']['streams'][0]['codec_name'], 'hevc')

    def test_incompatible_audio_rejected(self):
        self.run_cli(self.source, '--profile', 'compatible', code=2)

    def test_already_small_result_skipped(self):
        source = self.root / 'tiny.mkv'
        tool('ffmpeg', '-v', 'error', '-f', 'lavfi', '-i', 'color=size=320x240:rate=24',
             '-t', '2', '-c:v', 'libx264', '-crf', '40', str(source))
        out = self.root / 'rejected.mp4'
        p = self.run_cli(source, '--profile', 'compatible', '--quality', 'strict', '-o', out, code=10)
        self.assertEqual(p['status'], 'skipped')
        self.assertFalse(out.exists())

    def test_subtitles_preserved(self):
        sub = self.root / 'sub.srt'
        sub.write_text('1\n00:00:00,000 --> 00:00:01,500\nHello\n')
        source = self.root / 'subtitles.mkv'
        tool('ffmpeg', '-v', 'error', '-i', str(self.source), '-i', str(sub), '-map', '0', '-map', '1',
             '-c', 'copy', '-metadata:s:s:0', 'language=eng', '-disposition:s:0', 'forced', str(source))
        out = self.root / 'subtitles-small.mkv'
        self.run_cli(source, '-o', out, '--effort', 'quick')
        info = self.run_cli('inspect', out)
        self.assertEqual(info['media']['streams'][2]['codec_name'], 'subrip')
        self.assertEqual(info['media']['streams'][2]['disposition']['forced'], 1)

    def test_hdr_and_odd_dimensions_rejected(self):
        for name, filters, opts in [
            ('hdr', 'color=size=320x240', ['-pix_fmt', 'yuv420p10le', '-color_trc', 'smpte2084']),
            ('odd', 'testsrc=size=321x241', ['-pix_fmt', 'yuv444p']),
        ]:
            source = self.root / f'{name}.mkv'
            tool('ffmpeg', '-v', 'error', '-f', 'lavfi', '-i', filters, '-t', '1', '-c:v', 'ffv1', *opts, str(source))
            self.run_cli(source, code=2)

    def test_corrupt_and_cli_errors(self):
        p = self.root / 'broken.mp4'
        p.write_bytes(b'not video')
        self.run_cli(p, code=2)
        self.run_cli(self.source, '--effort', 'magic', code=2)
        self.run_cli(self.source, '-o', self.source, code=2)

    def test_missing_dependencies(self):
        p = subprocess.run([str(BIN), 'doctor', '--json'], env={**os.environ, 'PATH': ''}, capture_output=True, text=True)
        self.assertEqual(p.returncode, 3)
        self.assertEqual(json.loads(p.stdout)['status'], 'error')

    @unittest.skipUnless(os.name == 'posix', 'Native Unix filename bytes')
    def test_non_utf8_filename(self):
        path = os.fsencode(self.root) + b'/invalid-\xff.mkv'
        os.link(os.fsencode(self.source), path)
        p = subprocess.run([os.fsencode(BIN), path, b'--dry-run', b'--json'], capture_output=True)
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual(json.loads(p.stdout)['status'], 'planned')

    def test_multiple_audio_and_chapters(self):
        meta = self.root / 'chapters.txt'
        meta.write_text(';FFMETADATA1\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=1800\ntitle=Intro\n')
        source = self.root / 'chapters.mkv'
        tool('ffmpeg', '-v', 'error', '-i', str(self.source), '-f', 'ffmetadata', '-i', str(meta),
             '-map', '0:v', '-map', '0:a', '-map', '0:a', '-map_chapters', '1', '-c', 'copy',
             '-metadata:s:a:0', 'language=eng', '-metadata:s:a:1', 'language=fra', str(source))
        out = self.root / 'chapters-small.mkv'
        self.run_cli(source, '-o', out, '--effort', 'quick')
        info = self.run_cli('inspect', out)['media']
        self.assertEqual(len(info['streams']), 3)
        self.assertEqual(info['chapters'][0]['tags']['title'], 'Intro')

    def test_batch_and_config(self):
        directory = self.root / 'batch'
        directory.mkdir()
        source = directory / 'one.mkv'
        tool('ffmpeg', '-v', 'error', '-i', str(self.source), '-map', '0:v', '-c:v', 'copy', str(source))
        (directory / 'bad.mp4').write_bytes(b'broken')
        config = self.root / 'config.json'
        config.write_text(json.dumps({'profile':'modern','effort':'quick','threads':2}))
        p = self.run_cli('batch', directory, '--config', config, '--profile', 'compatible', code=4)
        self.assertEqual(p['status'], 'batch_partial')
        self.assertEqual(p['failed'], 1)
        self.assertTrue((directory / 'one.optimized.mp4').is_file())
        self.assertEqual((directory / 'bad.mp4').read_bytes(), b'broken')

    def test_quality_search_preview_and_full_verify(self):
        caps = self.run_cli('doctor')
        if not caps['vmaf_available']:
            self.skipTest('FFmpeg lacks libvmaf')
        source = self.root / 'quality-source.mkv'
        tool('ffmpeg', '-v', 'error', '-i', str(self.source), '-map', '0:v', '-c:v', 'copy', str(source))
        preview = self.root / 'preview'
        preview.mkdir()
        p = self.run_cli(source, '--profile', 'compatible', '--effort', 'quick', '--analyze', '--preview', preview)
        self.assertEqual(p['quality_verification'], 'sample_vmaf')
        self.assertTrue((preview / 'sample-0-source.mkv').is_file())
        self.assertTrue((preview / 'sample-0-encoded.mkv').is_file())
        before = (preview / 'sample-0-source.mkv').read_bytes()
        self.run_cli(source, '--profile', 'compatible', '--effort', 'quick', '--analyze', '--preview', preview, code=4)
        self.assertEqual((preview / 'sample-0-source.mkv').read_bytes(), before)
        out = self.root / 'quality.mp4'
        p = self.run_cli(source, '--profile', 'compatible', '--effort', 'quick', '--auto', '--verify', '-o', out)
        self.assertEqual(p['quality_verification'], 'full_vmaf')
        self.assertEqual(p['vmaf']['frames'], 48)
        self.assertTrue(out.is_file())

    def test_cancellation(self):
        out = self.root / 'cancelled.mkv'
        p = subprocess.Popen([str(BIN), str(self.source), '-o', str(out), '--json'], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        # Wait until output encoding starts (after probing and capabilities).
        deadline = time.monotonic() + 15
        while not list(self.root.glob('.vopt-*')):
            self.assertIsNone(p.poll())
            self.assertLess(time.monotonic(), deadline)
            time.sleep(0.01)
        p.send_signal(signal.SIGINT)
        stdout, stderr = p.communicate(timeout=10)
        self.assertEqual(p.returncode, 130, stdout + stderr)
        self.assertEqual(json.loads(stdout)['exit_code'], 130)
        self.assertFalse(out.exists())


if __name__ == '__main__':
    unittest.main(verbosity=2)
