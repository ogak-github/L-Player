import 'package:flutter/material.dart';

import '../player/fullscreen_controller.dart';
import '../player/player_controller.dart';
import '../player/playlist.dart';
import '../src/rust/api/player.dart';

String formatTime(double seconds) {
  final total = seconds.isFinite && seconds > 0 ? seconds.floor() : 0;
  final h = total ~/ 3600;
  final m = (total % 3600) ~/ 60;
  final s = (total % 60).toString().padLeft(2, '0');
  return h > 0 ? '$h:${m.toString().padLeft(2, '0')}:$s' : '$m:$s';
}

/// e.g. "English · ENG · subrip", falling back to the file name or id.
String trackLabel(MediaTrack track) {
  final name =
      track.title ??
      (track.externalFilename == null
          ? null
          : fileNameOf(track.externalFilename!));
  final parts = [
    ?name,
    if (track.language case final language?) language.toUpperCase(),
    ?track.codec,
  ];
  return parts.isEmpty ? 'Track ${track.id}' : parts.join(' · ');
}

String _formatStep(double step) =>
    step == step.roundToDouble() ? '${step.toInt()}s' : '${step}s';

class ControlsBar extends StatelessWidget {
  const ControlsBar({
    super.key,
    required this.controller,
    required this.fullscreen,
    this.transparent = false,
    this.playlistVisible = false,
    this.onTogglePlaylist,
  });

  final PlayerController controller;
  final FullscreenController fullscreen;

  /// Floating over the video in fullscreen, so no background of its own.
  final bool transparent;

  final bool playlistVisible;

  /// Shows the playlist button when set.
  final VoidCallback? onTogglePlaylist;

  @override
  Widget build(BuildContext context) {
    return Material(
      color: transparent
          ? Colors.transparent
          : Theme.of(context).colorScheme.surfaceContainer,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(12, 4, 12, 8),
        child: ListenableBuilder(
          listenable: Listenable.merge([
            controller,
            controller.state,
            controller.playlist,
            fullscreen,
          ]),
          builder: (context, _) {
            final state = controller.state.value;
            return Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                _SeekBar(controller: controller, state: state),
                _Buttons(
                  controller: controller,
                  fullscreen: fullscreen,
                  playlistVisible: playlistVisible,
                  onTogglePlaylist: onTogglePlaylist,
                  state: state,
                ),
              ],
            );
          },
        ),
      ),
    );
  }
}

class _SeekBar extends StatefulWidget {
  const _SeekBar({required this.controller, required this.state});

  final PlayerController controller;
  final PlayerState? state;

  @override
  State<_SeekBar> createState() => _SeekBarState();
}

class _SeekBarState extends State<_SeekBar> {
  /// Value under the thumb while dragging, so mpv updates don't fight the user.
  double? _dragValue;

  @override
  Widget build(BuildContext context) {
    final state = widget.state;
    final duration = state == null || state.idle ? 0.0 : state.duration;
    final position = (_dragValue ?? state?.position ?? 0.0).clamp(
      0.0,
      duration,
    );
    final textStyle = Theme.of(context).textTheme.bodySmall;

    return Row(
      children: [
        SizedBox(
          width: 64,
          child: Text(formatTime(position), style: textStyle),
        ),
        Expanded(
          child: Slider(
            value: duration > 0 ? position : 0,
            max: duration > 0 ? duration : 1,
            onChanged: duration > 0
                ? (value) => setState(() => _dragValue = value)
                : null,
            onChangeEnd: (value) {
              widget.controller.seekTo(value);
              setState(() => _dragValue = null);
            },
          ),
        ),
        SizedBox(
          width: 64,
          child: Text(
            duration > 0 ? formatTime(duration) : '--:--',
            textAlign: TextAlign.end,
            style: textStyle,
          ),
        ),
      ],
    );
  }
}

/// Core playback buttons sit in the middle and are bigger; helper buttons
/// (seek step, speed, tracks, volume, view) are smaller and muted on the sides.
class _Buttons extends StatelessWidget {
  const _Buttons({
    required this.controller,
    required this.fullscreen,
    required this.state,
    required this.playlistVisible,
    required this.onTogglePlaylist,
  });

  /// Room needed to keep the core buttons exactly centered.
  static const _centeredMinWidth = 1060.0;

  /// Below this everything no longer fits on one line.
  static const _singleLineMinWidth = 920.0;

  final PlayerController controller;
  final FullscreenController fullscreen;
  final bool playlistVisible;
  final VoidCallback? onTogglePlaylist;
  final PlayerState? state;

  @override
  Widget build(BuildContext context) {
    final core = _core(context);
    final left = _Secondary(child: _leftHelpers(context));
    final right = _Secondary(child: _rightHelpers(context));

    return LayoutBuilder(
      builder: (context, constraints) {
        if (constraints.maxWidth >= _centeredMinWidth) {
          return Row(
            children: [
              Expanded(
                child: Align(alignment: Alignment.centerLeft, child: left),
              ),
              core,
              Expanded(
                child: Align(alignment: Alignment.centerRight, child: right),
              ),
            ],
          );
        }
        if (constraints.maxWidth >= _singleLineMinWidth) {
          return Row(
            children: [left, const Spacer(), core, const Spacer(), right],
          );
        }
        return Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            core,
            Wrap(
              alignment: WrapAlignment.center,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [left, right],
            ),
          ],
        );
      },
    );
  }

  Widget _core(BuildContext context) {
    final state = this.state;
    final playing = state != null && !state.idle && !state.paused;
    final step = _formatStep(controller.seekStep);
    final colors = Theme.of(context).colorScheme;

    return IconButtonTheme(
      data: IconButtonThemeData(
        style: IconButton.styleFrom(
          iconSize: 28,
          foregroundColor: colors.onSurface,
        ),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          IconButton(
            tooltip: 'Previous (P)',
            icon: const Icon(Icons.skip_previous_rounded),
            onPressed: controller.previous,
          ),
          IconButton(
            tooltip: 'Back $step (←)',
            icon: const Icon(Icons.fast_rewind_rounded),
            onPressed: controller.seekBackward,
          ),
          const SizedBox(width: 4),
          IconButton.filled(
            tooltip: playing ? 'Pause (Space)' : 'Play (Space)',
            iconSize: 36,
            style: IconButton.styleFrom(foregroundColor: colors.onPrimary),
            icon: Icon(
              playing ? Icons.pause_rounded : Icons.play_arrow_rounded,
            ),
            onPressed: controller.playPause,
          ),
          const SizedBox(width: 4),
          IconButton(
            tooltip: 'Forward $step (→)',
            icon: const Icon(Icons.fast_forward_rounded),
            onPressed: controller.seekForward,
          ),
          IconButton(
            tooltip: 'Next (N)',
            icon: const Icon(Icons.skip_next_rounded),
            onPressed: controller.next,
          ),
          IconButton(
            tooltip: 'Stop',
            icon: const Icon(Icons.stop_rounded),
            onPressed: controller.stop,
          ),
        ],
      ),
    );
  }

  /// Seek step and speed.
  Widget _leftHelpers(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final speed = state?.speed ?? 1;
    final speedChanged = (speed - 1).abs() > 0.001;

    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        PopupMenuButton<double>(
          tooltip: 'Seek step',
          initialValue: controller.seekStep,
          onSelected: (value) => controller.seekStep = value,
          itemBuilder: (context) => [
            for (final s in seekSteps)
              PopupMenuItem(value: s, child: Text('Seek ${_formatStep(s)}')),
          ],
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 10),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                const Icon(Icons.timer_outlined, size: 18),
                const SizedBox(width: 4),
                Text(_formatStep(controller.seekStep)),
              ],
            ),
          ),
        ),
        const _Separator(),
        IconButton(
          tooltip: 'Slower ([)',
          icon: const Icon(Icons.remove_rounded),
          onPressed: controller.slower,
        ),
        TextButton(
          style: TextButton.styleFrom(
            // Highlight a non default speed so it is not forgotten.
            foregroundColor: speedChanged
                ? colors.primary
                : colors.onSurfaceVariant,
            visualDensity: VisualDensity.compact,
          ),
          onPressed: () => controller.setSpeed(1),
          child: Text('${speed.toStringAsFixed(2)}x'),
        ),
        IconButton(
          tooltip: 'Faster (])',
          icon: const Icon(Icons.add_rounded),
          onPressed: controller.faster,
        ),
      ],
    );
  }

  /// Tracks, shuffle, volume and view toggles.
  Widget _rightHelpers(BuildContext context) {
    final colors = Theme.of(context).colorScheme;

    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        _AudioTrackMenu(controller: controller),
        _SubtitleMenu(controller: controller, state: state),
        IconButton(
          tooltip: 'Shuffle',
          isSelected: controller.playlist.shuffle,
          icon: const Icon(Icons.shuffle_rounded),
          selectedIcon: Icon(Icons.shuffle_on_rounded, color: colors.primary),
          onPressed: controller.toggleShuffle,
        ),
        const _Separator(),
        _VolumeControl(controller: controller, volume: state?.volume ?? 100),
        const _Separator(),
        if (onTogglePlaylist != null)
          IconButton(
            tooltip: playlistVisible
                ? 'Hide playlist (L)'
                : 'Show playlist (L)',
            isSelected: playlistVisible,
            icon: const Icon(Icons.playlist_play_rounded),
            selectedIcon: Icon(
              Icons.playlist_play_rounded,
              color: colors.primary,
            ),
            onPressed: onTogglePlaylist,
          ),
        IconButton(
          tooltip: fullscreen.isFullscreen
              ? 'Exit fullscreen (Esc)'
              : 'Fullscreen (F)',
          icon: Icon(
            fullscreen.isFullscreen
                ? Icons.fullscreen_exit_rounded
                : Icons.fullscreen_rounded,
          ),
          onPressed: fullscreen.toggle,
        ),
      ],
    );
  }
}

/// Smaller, muted styling for helper buttons.
class _Secondary extends StatelessWidget {
  const _Secondary({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final color = Theme.of(context).colorScheme.onSurfaceVariant;
    return IconButtonTheme(
      data: IconButtonThemeData(
        style: IconButton.styleFrom(
          iconSize: 20,
          foregroundColor: color,
          visualDensity: VisualDensity.compact,
        ),
      ),
      // Covers PopupMenuButton icons and plain icons like the volume one.
      child: IconTheme.merge(
        data: IconThemeData(size: 20, color: color),
        child: DefaultTextStyle.merge(
          style: TextStyle(color: color, fontSize: 13),
          child: child,
        ),
      ),
    );
  }
}

/// Thin vertical line between groups of buttons.
class _Separator extends StatelessWidget {
  const _Separator();

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      height: 24,
      child: VerticalDivider(
        width: 17,
        thickness: 1,
        color: Theme.of(context).colorScheme.outlineVariant,
      ),
    );
  }
}

class _AudioTrackMenu extends StatelessWidget {
  const _AudioTrackMenu({required this.controller});

  final PlayerController controller;

  @override
  Widget build(BuildContext context) {
    final tracks = controller.audioTracks;
    return PopupMenuButton<int>(
      tooltip: 'Audio track',
      enabled: tracks.isNotEmpty,
      icon: const Icon(Icons.audiotrack_rounded),
      // 0 is never a valid mpv track id, use it for "off".
      onSelected: (id) => controller.selectAudioTrack(id == 0 ? null : id),
      itemBuilder: (context) => [
        for (final track in tracks)
          CheckedPopupMenuItem(
            value: track.id,
            checked: track.selected,
            child: Text(trackLabel(track)),
          ),
        const PopupMenuDivider(),
        CheckedPopupMenuItem(
          value: 0,
          checked: !tracks.any((t) => t.selected),
          child: const Text('Off'),
        ),
      ],
    );
  }
}

class _SubtitleMenu extends StatelessWidget {
  const _SubtitleMenu({required this.controller, required this.state});

  static const _off = 0;
  static const _loadFile = -1;

  final PlayerController controller;
  final PlayerState? state;

  @override
  Widget build(BuildContext context) {
    final tracks = controller.subtitleTracks;
    final showing =
        (state?.subtitlesVisible ?? true) && tracks.any((t) => t.selected);
    return PopupMenuButton<int>(
      tooltip: 'Subtitles',
      icon: Icon(
        showing ? Icons.subtitles_rounded : Icons.subtitles_off_outlined,
      ),
      onSelected: (value) => switch (value) {
        _loadFile => controller.pickSubtitles(),
        _off => controller.selectSubtitleTrack(null),
        final id => controller.selectSubtitleTrack(id),
      },
      itemBuilder: (context) => [
        for (final track in tracks)
          CheckedPopupMenuItem(
            value: track.id,
            checked: track.selected,
            child: Text(trackLabel(track)),
          ),
        if (tracks.isNotEmpty) const PopupMenuDivider(),
        CheckedPopupMenuItem(
          value: _off,
          checked: !tracks.any((t) => t.selected),
          child: const Text('Off'),
        ),
        const PopupMenuItem(
          value: _loadFile,
          child: ListTile(
            leading: Icon(Icons.file_open_outlined),
            title: Text('Load subtitle file…'),
            contentPadding: EdgeInsets.zero,
          ),
        ),
      ],
    );
  }
}

class _VolumeControl extends StatelessWidget {
  const _VolumeControl({required this.controller, required this.volume});

  final PlayerController controller;
  final double volume;

  @override
  Widget build(BuildContext context) {
    final icon = volume <= 0
        ? Icons.volume_off_rounded
        : volume < 50
        ? Icons.volume_down_rounded
        : Icons.volume_up_rounded;
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Icon(icon, size: 20),
        SizedBox(
          width: 120,
          child: Slider(
            value: volume.clamp(0.0, 100.0),
            max: 100,
            onChanged: controller.setVolume,
          ),
        ),
      ],
    );
  }
}
