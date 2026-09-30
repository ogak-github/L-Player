import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:window_manager/window_manager.dart';

bool get _isDesktop =>
    Platform.isLinux || Platform.isMacOS || Platform.isWindows;

/// Fullscreen for the whole window on desktop, immersive mode on mobile.
class FullscreenController extends ChangeNotifier with WindowListener {
  FullscreenController() {
    if (_isDesktop) windowManager.addListener(this);
  }

  var _isFullscreen = false;
  bool get isFullscreen => _isFullscreen;

  Future<void> toggle() => set(!_isFullscreen);

  Future<void> exit() => set(false);

  Future<void> set(bool fullscreen) async {
    if (fullscreen == _isFullscreen) return;
    if (_isDesktop) {
      await windowManager.setFullScreen(fullscreen);
    } else {
      await SystemChrome.setEnabledSystemUIMode(
        fullscreen ? SystemUiMode.immersiveSticky : SystemUiMode.edgeToEdge,
      );
    }
    _update(fullscreen);
  }

  void _update(bool fullscreen) {
    if (fullscreen == _isFullscreen) return;
    _isFullscreen = fullscreen;
    notifyListeners();
  }

  // Keep in sync when the window manager changes it (e.g. a system shortcut).
  @override
  void onWindowEnterFullScreen() => _update(true);

  @override
  void onWindowLeaveFullScreen() => _update(false);

  @override
  void dispose() {
    if (_isDesktop) windowManager.removeListener(this);
    super.dispose();
  }
}
