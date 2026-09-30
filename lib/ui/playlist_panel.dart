import 'package:flutter/material.dart';

import '../player/player_controller.dart';
import '../player/playlist.dart';

const _itemExtent = 72.0;

class PlaylistPanel extends StatefulWidget {
  const PlaylistPanel({super.key, required this.controller});

  final PlayerController controller;

  @override
  State<PlaylistPanel> createState() => PlaylistPanelState();
}

class PlaylistPanelState extends State<PlaylistPanel> {
  final _listKey = GlobalKey();
  final _scrollController = ScrollController();

  PlaylistController get _playlist => widget.controller.playlist;

  /// Playlist item under [globalPosition], used to attach a dropped subtitle
  /// to the item it was dropped on.
  MediaItem? itemAt(Offset globalPosition) {
    final box = _listKey.currentContext?.findRenderObject() as RenderBox?;
    if (box == null || !box.hasSize) return null;
    final local = box.globalToLocal(globalPosition);
    if (!(Offset.zero & box.size).contains(local)) return null;
    final offset = _scrollController.hasClients
        ? _scrollController.offset
        : 0.0;
    final index = ((local.dy + offset) / _itemExtent).floor();
    final items = _playlist.items;
    return index >= 0 && index < items.length ? items[index] : null;
  }

  @override
  void dispose() {
    _scrollController.dispose();
    super.dispose();
  }

  Future<void> _addUrl() async {
    final url = await showDialog<String>(
      context: context,
      builder: (context) => const _AddUrlDialog(),
    );
    if (url != null && url.trim().isNotEmpty) {
      await widget.controller.addUrl(url);
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return ListenableBuilder(
      listenable: _playlist,
      builder: (context, _) {
        final items = _playlist.items;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 8, 8, 8),
              child: Row(
                children: [
                  Expanded(
                    child: Text(
                      'Playlist (${items.length})',
                      style: theme.textTheme.titleMedium,
                    ),
                  ),
                  PopupMenuButton<VoidCallback>(
                    tooltip: 'Add to playlist',
                    icon: const Icon(Icons.add_rounded),
                    onSelected: (action) => action(),
                    itemBuilder: (context) => [
                      PopupMenuItem(
                        value: widget.controller.pickFiles,
                        child: const ListTile(
                          leading: Icon(Icons.file_open_outlined),
                          title: Text('Files…'),
                          contentPadding: EdgeInsets.zero,
                        ),
                      ),
                      PopupMenuItem(
                        value: widget.controller.pickFolder,
                        child: const ListTile(
                          leading: Icon(Icons.folder_open_outlined),
                          title: Text('Folder…'),
                          contentPadding: EdgeInsets.zero,
                        ),
                      ),
                      PopupMenuItem(
                        value: _addUrl,
                        child: const ListTile(
                          leading: Icon(Icons.link_rounded),
                          title: Text('URL…'),
                          contentPadding: EdgeInsets.zero,
                        ),
                      ),
                    ],
                  ),
                  IconButton(
                    tooltip: 'Clear playlist',
                    icon: const Icon(Icons.delete_sweep_outlined),
                    onPressed: items.isEmpty
                        ? null
                        : () async {
                            await widget.controller.stop();
                            _playlist.clear();
                          },
                  ),
                ],
              ),
            ),
            const Divider(height: 1),
            Expanded(
              child: items.isEmpty
                  ? const _EmptyPlaylist()
                  : ReorderableListView.builder(
                      key: _listKey,
                      scrollController: _scrollController,
                      itemExtent: _itemExtent,
                      buildDefaultDragHandles: false,
                      itemCount: items.length,
                      onReorderItem: _playlist.reorder,
                      itemBuilder: (context, index) => _PlaylistTile(
                        key: ValueKey(items[index].id),
                        index: index,
                        item: items[index],
                        controller: widget.controller,
                      ),
                    ),
            ),
          ],
        );
      },
    );
  }
}

class _PlaylistTile extends StatelessWidget {
  const _PlaylistTile({
    super.key,
    required this.index,
    required this.item,
    required this.controller,
  });

  final int index;
  final MediaItem item;
  final PlayerController controller;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final isCurrent = controller.playlist.currentId == item.id;
    final details = [
      if (item.isUrl) 'Stream',
      if (item.subtitles.isNotEmpty)
        '${item.subtitles.length} subtitle${item.subtitles.length > 1 ? 's' : ''}',
    ].join(' · ');

    return ListTile(
      selected: isCurrent,
      selectedTileColor: colors.primary.withValues(alpha: 0.12),
      contentPadding: const EdgeInsets.only(left: 4, right: 4),
      leading: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          ReorderableDragStartListener(
            index: index,
            child: const MouseRegion(
              cursor: SystemMouseCursors.grab,
              child: Padding(
                padding: EdgeInsets.all(4),
                child: Icon(Icons.drag_indicator_rounded, size: 20),
              ),
            ),
          ),
          Icon(
            isCurrent
                ? Icons.graphic_eq_rounded
                : item.isUrl
                ? Icons.link_rounded
                : item.isAudio
                ? Icons.music_note_rounded
                : Icons.movie_outlined,
            size: 20,
          ),
        ],
      ),
      title: Text(item.title, maxLines: 1, overflow: TextOverflow.ellipsis),
      subtitle: details.isEmpty
          ? null
          : Text(details, maxLines: 1, overflow: TextOverflow.ellipsis),
      trailing: IconButton(
        tooltip: 'Remove',
        icon: const Icon(Icons.close_rounded, size: 18),
        onPressed: () => controller.remove(item),
      ),
      onTap: () => controller.playItem(item),
    );
  }
}

class _EmptyPlaylist extends StatelessWidget {
  const _EmptyPlaylist();

  @override
  Widget build(BuildContext context) {
    final color = Theme.of(context).colorScheme.onSurface
        .withValues(alpha: 0.5);
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.playlist_add_rounded, size: 48, color: color),
            const SizedBox(height: 8),
            Text(
              'Drag and drop files or folders here,\nor use the + button.',
              textAlign: TextAlign.center,
              style: TextStyle(color: color),
            ),
          ],
        ),
      ),
    );
  }
}

class _AddUrlDialog extends StatefulWidget {
  const _AddUrlDialog();

  @override
  State<_AddUrlDialog> createState() => _AddUrlDialogState();
}

class _AddUrlDialogState extends State<_AddUrlDialog> {
  final _text = TextEditingController();

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  void _submit() => Navigator.of(context).pop(_text.text);

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Play from URL'),
      content: SizedBox(
        width: 420,
        child: TextField(
          controller: _text,
          autofocus: true,
          decoration: const InputDecoration(
            hintText: 'https://example.com/video.mp4',
          ),
          onSubmitted: (_) => _submit(),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton(onPressed: _submit, child: const Text('Add')),
      ],
    );
  }
}
