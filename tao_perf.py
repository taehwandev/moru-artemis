"""Bounded graphics evidence for the host-driven MCP adapter."""

import asyncio
import io
import math
import re
import subprocess
import sys
import tempfile
from pathlib import Path
from uuid import uuid4

import imageio_ffmpeg
from mcp.server.fastmcp import Image
from PIL import Image as PILImage, ImageDraw
from tao_adb import selected_adb_device


_PACKAGE = re.compile(r'[A-Za-z_][A-Za-z0-9_.]{0,199}')
_STAGES = {
    'input_to_traversal': ('HandleInputStart', 'PerformTraversalsStart'),
    'measure_layout': ('PerformTraversalsStart', 'DrawStart'),
    'draw_recording': ('DrawStart', 'SyncStart'),
    'render_thread': ('SyncStart', 'SwapBuffers'),
    'swap': ('SwapBuffers', 'FrameCompleted'),
}


def _profile_rows(output):
    """Use the header in each framestats block; Android adds columns over time."""
    for block in output.split('---PROFILEDATA---')[1::2]:
        lines = [line.strip().strip(',') for line in block.splitlines() if line.strip()]
        if not lines:
            continue
        names = lines[0].split(',')
        if 'IntendedVsync' not in names or 'FrameCompleted' not in names:
            continue
        for line in lines[1:]:
            values = line.split(',')
            if len(values) != len(names):
                continue
            try:
                yield dict(zip(names, map(int, values)))
            except ValueError:
                continue


def _parse_gfxinfo(output, include_stages=True):
    """Summarize one package's gfxinfo without exposing its raw dump."""
    def percentiles(values, levels=(50, 90, 99)):
        if not values:
            return None
        ordered = sorted(values)
        return {f'p{level}': round(ordered[math.ceil(len(ordered) * level / 100) - 1], 2)
                for level in levels}

    def count(pattern):
        match = re.search(pattern, output, re.MULTILINE)
        return int(match.group(1)) if match else None

    total = count(r'^\s*Total frames rendered:\s*(\d+)')
    janky = count(r'^\s*Janky frames:\s*(\d+)')
    if total is None or janky is None:
        raise RuntimeError('gfxinfo did not contain a frame summary for this package')
    result = {
        'total_frames': total,
        'janky_frames': janky,
        'jank_percent': round(janky * 100 / total, 2) if total else 0.0,
        'missed_vsync': count(r'^\s*Number Missed Vsync:\s*(\d+)'),
        'slow_ui_thread': count(r'^\s*Number Slow UI thread:\s*(\d+)'),
        'slow_draw_commands': count(r'^\s*Number Slow issue draw commands:\s*(\d+)'),
    }
    summary_percentiles = {
        f'p{level}': count(rf'^\s*{level}th percentile:\s*(\d+)ms')
        for level in (50, 90, 95, 99)
    }
    result['frame_time_ms'] = (summary_percentiles if any(
        value is not None for value in summary_percentiles.values()) else None)

    if include_stages:
        durations = {name: [] for name in _STAGES}
        frame_durations = []
        for row in _profile_rows(output):
            start, end = row.get('IntendedVsync', 0), row.get('FrameCompleted', 0)
            if 0 < start < end and end - start < 1_000_000_000:
                frame_durations.append((end - start) / 1_000_000)
            for name, (first, last) in _STAGES.items():
                begin, finish = row.get(first, 0), row.get(last, 0)
                if 0 < begin < finish and finish - begin < 1_000_000_000:
                    durations[name].append((finish - begin) / 1_000_000)
        result['framestats_frames'] = len(frame_durations)
        result['stage_ms'] = {name: percentiles(values) for name, values in durations.items()}
        fallback = percentiles(frame_durations, (50, 90, 95, 99))
        if fallback is not None:
            result['frame_time_ms'] = {
                name: value if value is not None else fallback[name]
                for name, value in summary_percentiles.items()}
    return result


def _record_sheet(device, seconds, fps, crop):
    remote = f'/sdcard/moru-artemis-{uuid4().hex}.mp4'
    try:
        with tempfile.TemporaryDirectory(prefix='tao-frames-') as directory:
            local = Path(directory) / 'recording.mp4'
            response = device.shell(
                f'screenrecord --time-limit {seconds} --bit-rate 4000000 {remote}',
                timeout=seconds + 15)
            if 'INVALID_LAYER_STACK' in response:
                raise RuntimeError('Screen recording unavailable for the current display state')
            if 'ERROR:' in response or 'error:' in response:
                raise RuntimeError('Device screen recording failed')
            device.sync.pull(remote, local)
            if local.stat().st_size == 0:
                raise RuntimeError('Device screen recording produced an empty video')
            pattern = str(Path(directory) / 'frame-%03d.png')
            try:
                subprocess.run(
                    [imageio_ffmpeg.get_ffmpeg_exe(), '-hide_banner', '-loglevel', 'error',
                     '-i', str(local), '-vf', f'fps={fps}', '-frames:v', str(seconds * fps),
                     pattern], capture_output=True, check=True, timeout=30)
            except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as exc:
                raise RuntimeError('Frame extraction failed') from exc
            frames = sorted(Path(directory).glob('frame-*.png'))
            if not frames:
                raise RuntimeError('Screen recording contained no frames')
            sheet = PILImage.new('RGB', (1024, math.ceil(len(frames) / 4) * 256), 'black')
            draw = ImageDraw.Draw(sheet)
            for index, path in enumerate(frames):
                with PILImage.open(path) as source:
                    frame = source.convert('RGB')
                if crop is not None:
                    left, top, right, bottom = crop
                    if right > frame.width or bottom > frame.height:
                        raise ValueError('crop is outside the recorded frame')
                    frame = frame.crop((left, top, right, bottom))
                frame.thumbnail((256, 232))
                x, y = (index % 4) * 256, (index // 4) * 256
                sheet.paste(frame, (x + (256 - frame.width) // 2, y + 24))
                draw.text((x + 4, y + 4), f'{index / fps:.2f}s', fill='white')
            data = io.BytesIO()
            sheet.save(data, format='PNG', optimize=True)
            return data.getvalue()
    finally:
        original_error = sys.exception()
        try:
            device.shell(f'rm -f {remote}', timeout=5)
        except Exception as cleanup_error:
            if original_error is None:
                raise RuntimeError('Temporary device recording cleanup failed') from cleanup_error
            original_error.add_note('Temporary device recording cleanup also failed')


class PerformanceTools:
    def __init__(self, server):
        self.server = server

    async def perf_frame_stats(self, device_serial: str, package_name: str,
                               action: str = 'read', stages: bool = True) -> dict:
        """Reset or summarize gfxinfo frames; never return the raw device dump."""
        if not isinstance(package_name, str) or not _PACKAGE.fullmatch(package_name):
            raise ValueError('package_name must be an Android package identifier')
        if action not in {'read', 'reset'}:
            raise ValueError('action must be read or reset')
        device = selected_adb_device(self.server._adb, device_serial)
        command = f'dumpsys gfxinfo {package_name}'
        if action == 'reset':
            command += ' reset'
        elif stages:
            command += ' framestats'
        output = await asyncio.to_thread(
            device.shell, command, timeout=20)
        if 'No process found' in output:
            raise RuntimeError('No gfxinfo process found for this package')
        if action == 'reset':
            return {'status': 'reset', 'device_serial': device_serial,
                    'package_name': package_name}
        return {'device_serial': device_serial, 'package_name': package_name,
                **_parse_gfxinfo(output, include_stages=stages)}

    async def record_frames(self, device_serial: str, seconds: int = 4, fps: int = 4,
                            crop: list[int] | None = None) -> Image:
        """Record a short screen sequence and return one bounded PNG contact sheet."""
        if type(seconds) is not int or type(fps) is not int or not (1 <= seconds <= 10 and
                                                               1 <= fps <= 4 and seconds * fps <= 16):
            raise ValueError('seconds must be 1..10, fps 1..4, and seconds*fps at most 16')
        if crop is not None and (not isinstance(crop, list) or len(crop) != 4 or
                                 any(type(value) is not int for value in crop) or
                                 crop[0] < 0 or crop[1] < 0 or
                                 crop[2] <= crop[0] or crop[3] <= crop[1]):
            raise ValueError('crop must be [left, top, right, bottom] pixel bounds')
        device = selected_adb_device(self.server._adb, device_serial)
        return Image(data=await asyncio.to_thread(_record_sheet, device, seconds, fps, crop),
                     format='png')
