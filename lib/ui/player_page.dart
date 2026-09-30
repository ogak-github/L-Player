import 'dart:async';

import 'package:desktop_drop/desktop_drop.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../player/fullscreen_controller.dart';
import '../player/player_controller.dart';
import '../player/playlist.dart';
import 'controls_bar.dart';
import 'fullscreen_view.dart';
import 'playlist_panel.dart';
import 'video_view.dart';

/// Creates the native player once, then shows [PlayerPage].
class PlayerLoader extends StatefulWidget {
  const PlayerLoader({super.key});

  @override
  State<PlayerLoader> createState() => _PlayerLoaderState();
}

class _PlayerLoaderState extends State<PlayerLoader> {
  late final Future<PlayerController> _controller = PlayerController.create();

  @override
  void dispose() {
    _controller.then((controller) => controller.dispose(), onError: (_) {});
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return FutureBuilder<PlayerController>(
      future: _controller,
      builder: (context, snapshot) {
        if (snapshot.hasError) {
          return Scaffold(
            body: Center(
              child: Padding(
                padding: const EdgeInsets.all(24),
                child: Text(
                  'Failed to start the player:\n${snapshot.error}',
                  textAlign: TextAlign.center,
                ),
              ),
            ),
          );
        }
        final controller = snapshot.data;
        if (controller == null) {
          return const Scaffold(
            body: Center(child: CircularProgressIndicator()),
          );
        }
        return PlayerPage(controller: controller);
      },
    );
  }
}

class PlayerPage extends StatefulWidget {
  const PlayerPage({super.key, required this.controller});

  final PlayerController controller;

  @override
  State<PlayerPage> createState() => _PlayerPageState();
}

class _PlayerPageState extends State<PlayerPage> {
  final _playlistKey = GlobalKey<PlaylistPanelState>();
  final _fullscreen = FullscreenController();
  final _focusNode = FocusNode(debugLabel: 'player');
  late final StreamSubscription<String> _messages;
  var _dragging = false;
  var _showPlaylist = true;

  static const _playlistWidth = 340.0;

  void _togglePlaylist() => setState(() => _showPlaylist = !_showPlaylist);

  PlayerController get _controller => widget.controller;

  @override
  void initState() {
    super.initState();
    // The fullscreen button moves in the tree and loses focus, keep keyboard
    // shortcuts (Esc in particular) working by refocusing the page.
    _fullscreen.addListener(_focusNode.requestFocus);
    _messages = _controller.messages.listen((message) {
      ScaffoldMessenger.of(context)
        ..hideCurrentSnackBar()
        ..showSnackBar(SnackBar(content: Text(message)));
    });
  }

  @override
  void dispose() {
    _messages.cancel();
    _fullscreen.dispose();
    _focusNode.dispose();
    super.dispose();
  }

  Future<void> _onDrop(DropDoneDetails details) async {
    setState(() => _dragging = false);
    final target = _playlistKey.currentState?.itemAt(details.globalPosition);
    await _controller.addPaths(
      details.files.map((file) => file.path).toList(),
      target: target,
    );
  }

  void _changeVolume(double delta) {
    final volume = _controller.state.value?.volume ?? 100;
    _controller.setVolume((volume + delta).clamp(0.0, 100.0));
  }

  Map<ShortcutActivator, VoidCallback> get _shortcuts => {
    const SingleActivator(LogicalKeyboardKey.space): _controller.playPause,
    const SingleActivator(LogicalKeyboardKey.arrowRight):
        _controller.seekForward,
    const SingleActivator(LogicalKeyboardKey.arrowLeft):
        _controller.seekBackward,
    const SingleActivator(LogicalKeyboardKey.arrowUp): () => _changeVolume(5),
    const SingleActivator(LogicalKeyboardKey.arrowDown): () =>
        _changeVolume(-5),
    const SingleActivator(LogicalKeyboardKey.keyN): _controller.next,
    const SingleActivator(LogicalKeyboardKey.keyP): _controller.previous,
    const SingleActivator(LogicalKeyboardKey.keyS): _controller.toggleSubtitles,
    const SingleActivator(LogicalKeyboardKey.keyF): _fullscreen.toggle,
    const SingleActivator(LogicalKeyboardKey.escape): _fullscreen.exit,
    const SingleActivator(LogicalKeyboardKey.keyL): _togglePlaylist,
    const SingleActivator(LogicalKeyboardKey.bracketLeft): _controller.slower,
    const SingleActivator(LogicalKeyboardKey.bracketRight): _controller.faster,
  };

  @override
  Widget build(BuildContext context) {
    final video = VideoView(controller: _controller);
    final controls = ControlsBar(
      controller: _controller,
      fullscreen: _fullscreen,
      playlistVisible: _showPlaylist,
      onTogglePlaylist: _togglePlaylist,
    );
    final playlist = PlaylistPanel(key: _playlistKey, controller: _controller);

    return CallbackShortcuts(
      bindings: _shortcuts,
      child: Focus(
        focusNode: _focusNode,
        autofocus: true,
        child: Scaffold(
          body: DropTarget(
            onDragEntered: (_) => setState(() => _dragging = true),
            onDragExited: (_) => setState(() => _dragging = false),
            onDragDone: _onDrop,
            child: Stack(
              fit: StackFit.expand,
              children: [
                ListenableBuilder(
                  listenable: _fullscreen,
                  builder: (context, _) => _fullscreen.isFullscreen
                      // Fullscreen: video only, controls float on top.
                      ? FullscreenView(
                          video: video,
                          controls: ControlsBar(
                            controller: _controller,
                            fullscreen: _fullscreen,
                            transparent: true,
                          ),
                        )
                      : _buildLayout(video, controls, playlist),
                ),
                if (_dragging) const _DropOverlay(),
              ],
            ),
          ),
        ),
      ),
    );
  }

  Widget _buildLayout(Widget video, Widget controls, Widget playlist) {
    return LayoutBuilder(
      builder: (context, constraints) {
        if (constraints.maxWidth >= 900) {
          return Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Expanded(
                child: Column(
                  children: [
                    Expanded(child: video),
                    controls,
                  ],
                ),
              ),
              // Slides the panel out instead of rebuilding it, so it keeps
              // its scroll position.
              AnimatedContainer(
                duration: const Duration(milliseconds: 200),
                curve: Curves.easeOutCubic,
                width: _showPlaylist ? _playlistWidth + 1 : 0,
                child: ClipRect(
                  child: OverflowBox(
                    alignment: Alignment.centerLeft,
                    minWidth: _playlistWidth + 1,
                    maxWidth: _playlistWidth + 1,
                    child: Row(
                      children: [
                        const VerticalDivider(width: 1),
                        SizedBox(width: _playlistWidth, child: playlist),
                      ],
                    ),
                  ),
                ),
              ),
            ],
          );
        }
        return SafeArea(
          child: Column(
            children: [
              if (_showPlaylist)
                AspectRatio(aspectRatio: 16 / 9, child: video)
              else
                Expanded(child: video),
              controls,
              if (_showPlaylist) Expanded(child: playlist),
            ],
          ),
        );
      },
    );
  }
}

class _DropOverlay extends StatelessWidget {
  const _DropOverlay();

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return Positioned.fill(
      child: IgnorePointer(
        child: Container(
          margin: const EdgeInsets.all(8),
          decoration: BoxDecoration(
            color: colors.primary.withValues(alpha: 0.08),
            border: Border.all(color: colors.primary, width: 2),
            borderRadius: BorderRadius.circular(12),
          ),
          alignment: Alignment.center,
          child: Text(
            'Drop to add to the playlist\n'
            'Drop a subtitle on a playlist item to attach it',
            textAlign: TextAlign.center,
            style: TextStyle(color: colors.primary, fontSize: 16),
          ),
        ),
      ),
    );
  }
}
