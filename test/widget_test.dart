import 'package:flutter_test/flutter_test.dart';
import 'package:lplayer/player/playlist.dart';

void main() {
  group('PlaylistController', () {
    test('plays items in list order and stops at the end', () {
      final playlist = PlaylistController()..addAll(['/a.mp4', '/b.mp4']);
      final first = playlist.nextItem!;
      expect(first.title, 'a.mp4');
      playlist.currentId = first.id;
      final second = playlist.nextItem!;
      expect(second.title, 'b.mp4');
      playlist.currentId = second.id;
      expect(playlist.nextItem, isNull);
      expect(playlist.previousItem, first);
    });

    test('reorder follows onReorderItem indexes', () {
      final playlist = PlaylistController()
        ..addAll(['/a.mp4', '/b.mp4', '/c.mp4']);
      playlist.reorder(0, 2);
      expect(playlist.items.map((i) => i.title), ['b.mp4', 'c.mp4', 'a.mp4']);
    });

    test('shuffle keeps the current item and visits every item once', () {
      final playlist = PlaylistController()
        ..addAll([for (var i = 0; i < 20; i++) '/$i.mp4']);
      playlist.currentId = playlist.items[5].id;
      playlist.shuffle = true;

      final visited = <int>{playlist.currentId!};
      for (
        var next = playlist.nextItem;
        next != null;
        next = playlist.nextItem
      ) {
        expect(visited.add(next.id), isTrue);
        playlist.currentId = next.id;
      }
      expect(visited.length, 20);
    });

    test('matches subtitles to the longest file name prefix', () {
      final playlist = PlaylistController()
        ..addAll(['/v/Movie.mkv', '/v/Movie 2.mkv', 'https://x.io/Movie.mp4']);
      expect(
        playlist.itemMatchingSubtitle('/s/Movie 2.en.srt')!.uri,
        '/v/Movie 2.mkv',
      );
      expect(
        playlist.itemMatchingSubtitle('/s/Movie.srt')!.uri,
        '/v/Movie.mkv',
      );
      expect(playlist.itemMatchingSubtitle('/s/Other.srt'), isNull);
    });
  });

  test('detects file kinds', () {
    expect(isMedia('/x/Song.FLAC'), isTrue);
    expect(isSubtitle('/x/a.ass'), isTrue);
    expect(looksLikeUrl('https://example.com/a.m3u8'), isTrue);
    expect(looksLikeUrl('/home/a.mp4'), isFalse);
  });
}
