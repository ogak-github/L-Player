import 'dart:async';

import 'package:flutter/material.dart';

const _hideDelay = Duration(seconds: 3);

/// Video over the whole screen with [controls] floating on top. The controls
/// and the mouse cursor hide after [_hideDelay] without mouse movement.
class FullscreenView extends StatefulWidget {
  const FullscreenView({
    super.key,
    required this.video,
    required this.controls,
  });

  final Widget video;
  final Widget controls;

  @override
  State<FullscreenView> createState() => _FullscreenViewState();
}

class _FullscreenViewState extends State<FullscreenView> {
  var _visible = true;
  var _hoveringControls = false;
  Timer? _hideTimer;

  @override
  void initState() {
    super.initState();
    _scheduleHide();
  }

  @override
  void dispose() {
    _hideTimer?.cancel();
    super.dispose();
  }

  void _show() {
    if (!_visible) setState(() => _visible = true);
    _scheduleHide();
  }

  void _hide() {
    _hideTimer?.cancel();
    if (_visible) setState(() => _visible = false);
  }

  void _scheduleHide() {
    _hideTimer?.cancel();
    _hideTimer = Timer(_hideDelay, () {
      // Keep them up while the user is working with the controls.
      if (mounted && !_hoveringControls) _hide();
    });
  }

  @override
  Widget build(BuildContext context) {
    return MouseRegion(
      cursor: _visible ? MouseCursor.defer : SystemMouseCursors.none,
      onHover: (_) => _show(),
      child: GestureDetector(
        // Touch screens have no hover: tap toggles the controls.
        onTap: () => _visible ? _hide() : _show(),
        child: Stack(
          fit: StackFit.expand,
          children: [
            widget.video,
            Positioned(
              left: 0,
              right: 0,
              bottom: 0,
              child: IgnorePointer(
                ignoring: !_visible,
                child: AnimatedOpacity(
                  opacity: _visible ? 1 : 0,
                  duration: const Duration(milliseconds: 200),
                  child: MouseRegion(
                    onEnter: (_) => _hoveringControls = true,
                    onExit: (_) {
                      _hoveringControls = false;
                      _scheduleHide();
                    },
                    child: widget.controls,
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
