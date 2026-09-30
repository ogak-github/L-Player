import 'dart:io';

import 'package:flutter/material.dart';
import 'package:window_manager/window_manager.dart';

import 'src/rust/frb_generated.dart';
import 'ui/player_page.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await RustLib.init();
  if (Platform.isLinux || Platform.isMacOS || Platform.isWindows) {
    await windowManager.ensureInitialized();
  }
  runApp(const LPlayerApp());
}

class LPlayerApp extends StatelessWidget {
  const LPlayerApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'L-Player',
      debugShowCheckedModeBanner: false,
      themeMode: ThemeMode.dark,
      darkTheme: ThemeData(
        colorScheme: ColorScheme.fromSeed(
          seedColor: const Color(0xFF4F7CFF),
          brightness: Brightness.dark,
        ),
        useMaterial3: true,
      ),
      home: const PlayerLoader(),
    );
  }
}
