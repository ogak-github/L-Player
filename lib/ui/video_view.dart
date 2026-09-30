import 'package:flutter/material.dart';

import '../player/player_controller.dart';
import '../src/rust/api/player.dart';

/// Video texture letterboxed on black, or a placeholder when there is no picture.
class VideoView extends StatelessWidget {
  const VideoView({super.key, required this.controller});

  final PlayerController controller;

  @override
  Widget build(BuildContext context) {
    return ColoredBox(
      color: Colors.black,
      child: ValueListenableBuilder<PlayerState?>(
        valueListenable: controller.state,
        builder: (context, state, _) {
          final hasVideo =
              state != null &&
              !state.idle &&
              state.videoWidth > 0 &&
              state.videoHeight > 0;
          return Stack(
            fit: StackFit.expand,
            children: [
              if (hasVideo)
                Center(
                  child: AspectRatio(
                    aspectRatio: state!.videoWidth / state!.videoHeight,
                    child: Texture(
                      textureId: controller.textureId,
                      filterQuality: FilterQuality.medium,
                    ),
                  ),
                )
              else
                _Placeholder(state: state),
              if (state != null && state.buffering)
                const Center(child: CircularProgressIndicator()),
            ],
          );
        },
      ),
    );
  }
}

class _Placeholder extends StatelessWidget {
  const _Placeholder({required this.state});

  final PlayerState? state;

  @override
  Widget build(BuildContext context) {
    final playingAudio = state != null && !state!.idle;
    final colors = Theme.of(context).colorScheme;
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(
            playingAudio ? Icons.music_note_rounded : Icons.movie_outlined,
            size: 72,
            color: colors.onSurface.withValues(alpha: 0.3),
          ),
          const SizedBox(height: 12),
          Text(
            playingAudio
                ? state!.title
                : 'Drop videos, audio or subtitles here',
            textAlign: TextAlign.center,
            style: TextStyle(color: colors.onSurface.withValues(alpha: 0.6)),
          ),
        ],
      ),
    );
  }
}
