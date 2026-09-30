import 'dart:async';
import 'dart:io';

import 'package:file_selector/file_selector.dart';

import 'package:flutter/foundation.dart';
import 'package:irondash_engine_context/irondash_engine_context.dart';

import '../src/rust/api/player.dart';
import 'playlist.dart';

const seekSteps = [10.0, 5.0, 2.0, 1.0, 0.5];
const speedSteps = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 3.0, 4.0];

/// Glue between the Rust player and the playlist: auto advances on end of
/// file, routes dropped files and exposes simple actions to the UI.
class PlayerController extends ChangeNotifier {
  PlayerController._(this._player) {
    _events = _player.events().listen(_onEvent);
  }

  static Future<PlayerController> create() async {
    final engineHandle = await EngineContext.instance.getEngineHandle();
    final player = await createPlayer(engineHandle: engineHandle);
    return PlayerController._(player);
  }

  final LPlayer _player;
  late final StreamSubscription<PlayerEvent> _events;
  final playlist = PlaylistController();

  /// Latest state from mpv, null until the first event arrives.
  final state = ValueNotifier<PlayerState?>(null);

  final _messages = StreamController<String>.broadcast();

  /// Errors and hints to show to the user.
  Stream<String> get messages => _messages.stream;

  int get textureId => _player.textureId;

  double _seekStep = seekSteps.first;
  double get seekStep => _seekStep;
  set seekStep(double value) {
    _seekStep = value;
    notifyListeners();
  }

  void _onEvent(PlayerEvent event) {
    state.value = event.state;
    switch (event.kind) {
      case PlayerEventKind.endOfFile:
        next();
      case PlayerEventKind.error:
        final title = playlist.current?.title ?? 'file';
        _messages.add(
          'Failed to play $title: ${event.message ?? 'unknown error'}',
        );
        next();
      case PlayerEventKind.stateChanged || PlayerEventKind.fileLoaded:
        break;
    }
  }

  /// Runs a player call and reports failures instead of throwing.
  Future<void> _run(Future<void> Function() action) async {
    try {
      await action();
    } catch (e) {
      _messages.add(e.toString());
    }
  }

  Future<void> playItem(MediaItem item) async {
    playlist.currentId = item.id;
    await _run(() => _player.open(uri: item.uri, subtitles: item.subtitles));
  }

  /// Plays the next item, or stops at the end of the list.
  Future<void> next() async {
    final item = playlist.nextItem;
    if (item == null) {
      await stop();
      // Pressing play after the list finished starts over from the top.
      playlist.currentId = null;
      return;
    }
    await playItem(item);
  }

  Future<void> previous() async {
    // Like most players: restart the current file unless we are at its start.
    if ((state.value?.position ?? 0) > 3 && playlist.current != null) {
      await seekTo(0);
      return;
    }
    final item = playlist.previousItem;
    if (item != null) await playItem(item);
  }

  Future<void> playPause() async {
    final idle = state.value?.idle ?? true;
    if (!idle) return _run(_player.togglePause);
    final item = playlist.current ?? playlist.nextItem;
    if (item != null) await playItem(item);
  }

  Future<void> stop() => _run(_player.stop);

  Future<void> seekTo(double seconds) =>
      _run(() => _player.seekTo(seconds: seconds));

  Future<void> seekForward() => _run(() => _player.seekBy(seconds: _seekStep));

  Future<void> seekBackward() =>
      _run(() => _player.seekBy(seconds: -_seekStep));

  Future<void> setVolume(double volume) =>
      _run(() => _player.setVolume(volume: volume));

  Future<void> setSpeed(double speed) =>
      _run(() => _player.setSpeed(speed: speed));

  Future<void> faster() {
    final speed = state.value?.speed ?? 1;
    final next = speedSteps.firstWhere(
      (s) => s > speed + 0.001,
      orElse: () => speedSteps.last,
    );
    return setSpeed(next);
  }

  Future<void> slower() {
    final speed = state.value?.speed ?? 1;
    final next = speedSteps.lastWhere(
      (s) => s < speed - 0.001,
      orElse: () => speedSteps.first,
    );
    return setSpeed(next);
  }

  List<MediaTrack> _tracks(TrackKind kind) => [
    for (final track in state.value?.tracks ?? const <MediaTrack>[])
      if (track.kind == kind) track,
  ];

  List<MediaTrack> get audioTracks => _tracks(TrackKind.audio);
  List<MediaTrack> get subtitleTracks => _tracks(TrackKind.subtitle);

  /// [id] null turns audio off.
  Future<void> selectAudioTrack(int? id) =>
      _run(() => _player.selectAudioTrack(id: id));

  /// [id] null turns subtitles off.
  Future<void> selectSubtitleTrack(int? id) => _run(() async {
    await _player.selectSubtitleTrack(id: id);
    // Picking a track should show it even if subtitles were toggled off.
    if (id != null) await _player.setSubtitlesVisible(visible: true);
  });

  Future<void> toggleSubtitles() {
    final visible = state.value?.subtitlesVisible ?? true;
    return _run(() => _player.setSubtitlesVisible(visible: !visible));
  }

  void toggleShuffle() => playlist.shuffle = !playlist.shuffle;

  /// Adds dropped files. Media goes to the playlist; subtitles are attached to
  /// [target] if given, otherwise to the item with a matching name, otherwise
  /// to the current item.
  Future<void> addPaths(List<String> dropped, {MediaItem? target}) async {
    final paths = await _expandFolders(dropped);
    final wasEmpty = playlist.isEmpty;
    final added = playlist.addAll(
      paths.where((p) => isMedia(p) || looksLikeUrl(p)),
    );

    for (final subtitle in paths.where(isSubtitle)) {
      final item =
          target ?? playlist.itemMatchingSubtitle(subtitle) ?? playlist.current;
      if (item == null) {
        _messages.add(
          'Add a video first, then drop ${fileNameOf(subtitle)} on it',
        );
        continue;
      }
      await attachSubtitle(item, subtitle);
    }

    final ignored = paths.where(
      (p) => !isMedia(p) && !isSubtitle(p) && !looksLikeUrl(p),
    );
    if (ignored.isNotEmpty) {
      _messages.add('Unsupported file: ${ignored.map(fileNameOf).join(', ')}');
    }

    // Start right away when the list was empty and nothing is playing.
    if (wasEmpty && added.isNotEmpty && (state.value?.idle ?? true)) {
      await playItem(added.first);
    }
  }

  Future<void> addUrl(String url) => addPaths([url.trim()]);

  Future<void> pickFiles() async {
    final files = await openFiles(
      acceptedTypeGroups: [
        XTypeGroup(
          label: 'Media and subtitles',
          extensions: [
            ...videoExtensions,
            ...audioExtensions,
            ...subtitleExtensions,
          ],
        ),
        const XTypeGroup(label: 'All files'),
      ],
    );
    await addPaths([for (final file in files) file.path]);
  }

  Future<void> pickFolder() async {
    final folder = await getDirectoryPath();
    if (folder != null) await addPaths([folder]);
  }

  /// Lets the user pick subtitle files for the current item.
  Future<void> pickSubtitles() async {
    final item = playlist.current;
    if (item == null) {
      _messages.add('Play something first, then load its subtitles');
      return;
    }
    final files = await openFiles(
      acceptedTypeGroups: [
        XTypeGroup(label: 'Subtitles', extensions: subtitleExtensions.toList()),
      ],
    );
    for (final file in files) {
      await attachSubtitle(item, file.path);
    }
  }

  /// Replaces folders with the media and subtitle files inside them.
  Future<List<String>> _expandFolders(List<String> paths) async {
    final result = <String>[];
    for (final path in paths) {
      if (looksLikeUrl(path) || !await FileSystemEntity.isDirectory(path)) {
        result.add(path);
        continue;
      }
      try {
        final files = await Directory(path)
            .list(recursive: true)
            .where((entry) => entry is File)
            .map((entry) => entry.path)
            .where((p) => isMedia(p) || isSubtitle(p))
            .toList();
        result.addAll(files..sort());
      } on FileSystemException catch (e) {
        _messages.add('Cannot read folder $path: ${e.message}');
      }
    }
    return result;
  }

  Future<void> attachSubtitle(MediaItem item, String path) async {
    playlist.attachSubtitle(item.id, path);
    final playing = !(state.value?.idle ?? true);
    if (playing && playlist.currentId == item.id) {
      await _run(() => _player.addSubtitle(path: path));
    }
    _messages.add('Subtitle ${fileNameOf(path)} added to ${item.title}');
  }

  Future<void> remove(MediaItem item) async {
    if (playlist.currentId == item.id) await stop();
    playlist.remove(item.id);
  }

  @override
  void dispose() {
    _events.cancel();
    _messages.close();
    state.dispose();
    playlist.dispose();
    _player.dispose();
    super.dispose();
  }
}
