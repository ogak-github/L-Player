import 'dart:math';

import 'package:flutter/foundation.dart';

const videoExtensions = {
  'mp4', 'mkv', 'webm', 'avi', 'mov', 'm4v', 'wmv', 'flv', 'ts', 'm2ts', //
  'mpg', 'mpeg', '3gp', 'ogv',
};
const audioExtensions = {
  'mp3',
  'flac',
  'wav',
  'ogg',
  'opus',
  'm4a',
  'aac',
  'wma',
  'alac',
  'aiff',
};
const subtitleExtensions = {'srt', 'ass', 'ssa', 'vtt', 'sub', 'idx', 'sup'};

String extensionOf(String path) {
  final name = fileNameOf(path);
  final dot = name.lastIndexOf('.');
  return dot < 0 ? '' : name.substring(dot + 1).toLowerCase();
}

String fileNameOf(String path) {
  final clean = path.endsWith('/') ? path.substring(0, path.length - 1) : path;
  return clean.substring(clean.lastIndexOf('/') + 1);
}

String _baseNameOf(String path) {
  final name = fileNameOf(path);
  final dot = name.lastIndexOf('.');
  return (dot < 0 ? name : name.substring(0, dot)).toLowerCase();
}

bool isSubtitle(String path) => subtitleExtensions.contains(extensionOf(path));

bool isMedia(String path) {
  final ext = extensionOf(path);
  return videoExtensions.contains(ext) || audioExtensions.contains(ext);
}

bool looksLikeUrl(String uri) =>
    RegExp(r'^[a-zA-Z][a-zA-Z0-9+.-]*://').hasMatch(uri);

class MediaItem {
  MediaItem({required this.id, required this.uri}) : title = _titleFor(uri);

  final int id;

  /// Local file path or URL, passed to mpv as is.
  final String uri;
  final String title;

  /// External subtitle files attached by the user.
  final List<String> subtitles = [];

  bool get isUrl => looksLikeUrl(uri);
  bool get isAudio => audioExtensions.contains(extensionOf(uri));

  static String _titleFor(String uri) {
    if (!looksLikeUrl(uri)) return fileNameOf(uri);
    final parsed = Uri.tryParse(uri);
    final last = parsed?.pathSegments.where((s) => s.isNotEmpty).lastOrNull;
    return last ?? uri;
  }
}

/// Play queue: keeps the list order the user sees plus a separate shuffled
/// play order. Items are referenced by id so reordering and removing never
/// confuses what is currently playing.
class PlaylistController extends ChangeNotifier {
  final _items = <MediaItem>[];
  final _random = Random();
  var _nextId = 0;
  int? _currentId;
  var _shuffle = false;

  /// Play order while shuffling, ids of [_items].
  var _shuffleOrder = <int>[];

  List<MediaItem> get items => List.unmodifiable(_items);
  bool get isEmpty => _items.isEmpty;
  bool get shuffle => _shuffle;
  int? get currentId => _currentId;
  MediaItem? get current => _byId(_currentId);

  MediaItem? _byId(int? id) =>
      id == null ? null : _items.where((item) => item.id == id).firstOrNull;

  List<int> get _playOrder =>
      _shuffle ? _shuffleOrder : [for (final item in _items) item.id];

  List<MediaItem> addAll(Iterable<String> uris) {
    final added = [for (final uri in uris) MediaItem(id: _nextId++, uri: uri)];
    if (added.isEmpty) return added;
    _items.addAll(added);
    for (final item in added) {
      // New items land somewhere after the current one so they still get played.
      final start = _currentId == null
          ? 0
          : _shuffleOrder.indexOf(_currentId!) + 1;
      final index = start + _random.nextInt(_shuffleOrder.length - start + 1);
      _shuffleOrder.insert(index, item.id);
    }
    notifyListeners();
    return added;
  }

  void remove(int id) {
    _items.removeWhere((item) => item.id == id);
    _shuffleOrder.remove(id);
    if (_currentId == id) _currentId = null;
    notifyListeners();
  }

  void clear() {
    _items.clear();
    _shuffleOrder.clear();
    _currentId = null;
    notifyListeners();
  }

  /// [newIndex] is the final index, as given by `onReorderItem`.
  void reorder(int oldIndex, int newIndex) {
    final item = _items.removeAt(oldIndex);
    _items.insert(newIndex, item);
    notifyListeners();
  }

  set currentId(int? id) {
    if (_currentId == id) return;
    _currentId = id;
    notifyListeners();
  }

  set shuffle(bool value) {
    if (_shuffle == value) return;
    _shuffle = value;
    if (value) {
      // Keep the current item first so shuffling does not replay what was heard.
      final rest = [for (final item in _items) item.id]
        ..remove(_currentId)
        ..shuffle(_random);
      _shuffleOrder = [if (_currentId != null) _currentId!, ...rest];
    }
    notifyListeners();
  }

  MediaItem? get nextItem => _step(1);
  MediaItem? get previousItem => _step(-1);

  MediaItem? _step(int delta) {
    final order = _playOrder;
    if (order.isEmpty) return null;
    final index = _currentId == null ? -1 : order.indexOf(_currentId!);
    if (index < 0) return _byId(order.first);
    final target = index + delta;
    if (target < 0 || target >= order.length) return null;
    return _byId(order[target]);
  }

  void attachSubtitle(int id, String path) {
    final item = _byId(id);
    if (item == null || item.subtitles.contains(path)) return;
    item.subtitles.add(path);
    notifyListeners();
  }

  /// Local item whose file name the subtitle starts with,
  /// e.g. `Movie.en.srt` belongs to `Movie.mkv`.
  MediaItem? itemMatchingSubtitle(String path) {
    final subtitle = _baseNameOf(path);
    MediaItem? best;
    for (final item in _items.where((item) => !item.isUrl)) {
      final base = _baseNameOf(item.uri);
      if (subtitle.startsWith(base) &&
          (best == null || base.length > _baseNameOf(best.uri).length)) {
        best = item;
      }
    }
    return best;
  }
}
