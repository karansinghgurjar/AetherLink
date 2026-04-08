import 'dart:async';
import 'dart:convert';
import 'dart:ui' as ui;

import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'remote_client.dart';

void main() {
  runApp(const RemoteApp());
}

class RemoteApp extends StatelessWidget {
  const RemoteApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'AetherLink',
      theme: ThemeData(
        colorScheme: ColorScheme.fromSeed(seedColor: Colors.blue),
      ),
      home: const RemoteHomePage(),
    );
  }
}

enum ResolutionPreset { low, medium, high }
enum ClipboardSyncMode { manual, hostToClient, clientToHost, bidirectional }

String _clipboardModeToWire(ClipboardSyncMode mode) {
  return switch (mode) {
    ClipboardSyncMode.hostToClient => 'host_to_client',
    _ => 'manual',
  };
}

ClipboardSyncMode _clipboardModeFromWire(String? mode) {
  return switch (mode) {
    'host_to_client' => ClipboardSyncMode.hostToClient,
    _ => ClipboardSyncMode.manual,
  };
}

enum ConnectionStage {
  idle,
  connecting,
  tlsHandshake,
  authenticating,
  connected,
  reconnecting,
  authFailed,
  tlsFailed,
  disconnected,
  stopped,
  error,
}

enum TrustState {
  unpaired,
  pending,
  trusted,
  rejected,
  revoked,
}

enum StreamPhase {
  connecting,
  awaitingFirstFrame,
  streaming,
}

class _SettingsState {
  ResolutionPreset resolution = ResolutionPreset.medium;
  int fps = 15;
  double jpegQuality = 60;
  bool viewOnly = false;
  int monitorIndex = 0;
  ClipboardSyncMode clipboardMode = ClipboardSyncMode.manual;
  bool deltaStreamEnabled = true;
  bool audioEnabled = false;

  int get targetWidth {
    switch (resolution) {
      case ResolutionPreset.low:
        return 854;
      case ResolutionPreset.medium:
        return 960;
      case ResolutionPreset.high:
        return 1280;
    }
  }
}

class SavedHostEntry {
  const SavedHostEntry({
    required this.label,
    required this.host,
    required this.port,
    required this.token,
    required this.useRelayMode,
    required this.relayHostId,
    required this.relayToken,
    required this.resolution,
    required this.fps,
    required this.jpegQuality,
    required this.viewOnly,
    required this.monitorIndex,
    required this.autoReconnect,
    required this.clipboardMode,
    required this.deltaStreamEnabled,
    required this.audioEnabled,
  });

  final String label;
  final String host;
  final int port;
  final String token;
  final bool useRelayMode;
  final String relayHostId;
  final String relayToken;
  final ResolutionPreset resolution;
  final int fps;
  final double jpegQuality;
  final bool viewOnly;
  final int monitorIndex;
  final bool autoReconnect;
  final ClipboardSyncMode clipboardMode;
  final bool deltaStreamEnabled;
  final bool audioEnabled;

  Map<String, Object> toJson() => <String, Object>{
        'label': label,
        'host': host,
        'port': port,
        'token': token,
        'use_relay_mode': useRelayMode,
        'relay_host_id': relayHostId,
        'relay_token': relayToken,
        'resolution': resolution.name,
        'fps': fps,
        'jpeg_quality': jpegQuality,
        'view_only': viewOnly,
        'monitor_index': monitorIndex,
        'auto_reconnect': autoReconnect,
        'clipboard_mode': clipboardMode.name,
        'delta_stream_enabled': deltaStreamEnabled,
        'audio_enabled': audioEnabled,
      };

  factory SavedHostEntry.fromJson(Map<String, dynamic> json) {
    return SavedHostEntry(
      label: (json['label'] as String?)?.trim().isNotEmpty == true
          ? (json['label'] as String).trim()
          : 'Saved Host',
      host: (json['host'] as String?)?.trim() ?? '',
      port: (json['port'] as num?)?.toInt() ?? 6000,
      token: (json['token'] as String?) ?? '',
      useRelayMode: json['use_relay_mode'] == true,
      relayHostId: (json['relay_host_id'] as String?)?.trim() ?? 'default-host',
      relayToken: (json['relay_token'] as String?) ?? '',
      resolution: ResolutionPreset.values.firstWhere(
        (value) => value.name == (json['resolution'] as String?),
        orElse: () => ResolutionPreset.low,
      ),
      fps: (json['fps'] as num?)?.toInt() ?? 15,
      jpegQuality: (json['jpeg_quality'] as num?)?.toDouble() ?? 60,
      viewOnly: json['view_only'] == true,
      monitorIndex: (json['monitor_index'] as num?)?.toInt() ?? 0,
      autoReconnect: json['auto_reconnect'] != false,
      clipboardMode: ClipboardSyncMode.values.firstWhere(
        (value) => value == _clipboardModeFromWire(json['clipboard_mode'] as String?),
        orElse: () => ClipboardSyncMode.manual,
      ),
      deltaStreamEnabled: json['delta_stream_enabled'] != false,
      audioEnabled: json['audio_enabled'] == true,
    );
  }
}

class _LatestFramePresenter {
  _LatestFramePresenter({
    required this.onLog,
    required this.onPresented,
    required this.isAwaitingFirstFrame,
  });

  static const int staleFrameThresholdMs = 200;
  static const int startupStaleFrameThresholdMs = 5000;

  final void Function(String message) onLog;
  final void Function(RemoteVideoFrame frame, int renderedAgeMs) onPresented;
  final bool Function() isAwaitingFirstFrame;
  final ValueNotifier<ui.Image?> imageListenable = ValueNotifier<ui.Image?>(null);
  final ValueNotifier<Size?> frameSizeListenable = ValueNotifier<Size?>(null);

  RemoteVideoFrame? _pendingFrame;
  bool _rendering = false;
  int _replacedPendingCount = 0;

  void submit(RemoteVideoFrame frame) {
    final awaitingFirstFrame = isAwaitingFirstFrame();
    final staleThreshold = awaitingFirstFrame ? startupStaleFrameThresholdMs : staleFrameThresholdMs;
    if ((!awaitingFirstFrame || !kDisableStartupStaleDrop) && frame.queueAgeMs > staleThreshold) {
      onLog(
        'Render stale-frame drop: frame ${frame.frameId} queueAge=${frame.queueAgeMs}ms hostClockAge=${frame.hostClockAgeMs}ms threshold=${staleThreshold}ms phase=${awaitingFirstFrame ? 'awaiting_first_frame' : 'streaming'}',
      );
      return;
    }
    if (_rendering) {
      if (_pendingFrame != null) {
        _replacedPendingCount += 1;
        if (frame.frameId % 30 == 0) {
          onLog(
            'Render backlog replace: old ${_pendingFrame!.frameId} -> new ${frame.frameId} total=$_replacedPendingCount',
          );
        }
      }
      _pendingFrame = frame;
      return;
    }
    _renderLatest(frame);
  }

  void clear() {
    _pendingFrame = null;
    _rendering = false;
    final previous = imageListenable.value;
    imageListenable.value = null;
    frameSizeListenable.value = null;
    previous?.dispose();
  }

  void dispose() {
    clear();
    imageListenable.dispose();
    frameSizeListenable.dispose();
  }

  void _renderLatest(RemoteVideoFrame frame) {
    _rendering = true;
    final renderStartMs = DateTime.now().millisecondsSinceEpoch;
    ui.decodeImageFromPixels(
      frame.rgbaBytes,
      frame.width,
      frame.height,
      ui.PixelFormat.rgba8888,
      (ui.Image image) {
        final awaitingFirstFrame = isAwaitingFirstFrame();
        final staleThreshold = awaitingFirstFrame ? startupStaleFrameThresholdMs : staleFrameThresholdMs;
        final renderedAgeMs = DateTime.now().millisecondsSinceEpoch - frame.receivedTimestampMs;
        if ((!awaitingFirstFrame || !kDisableStartupStaleDrop) && renderedAgeMs > staleThreshold) {
          image.dispose();
          onLog(
            'Render stale-frame drop: frame ${frame.frameId} queueAge=${renderedAgeMs}ms hostClockAge=${frame.hostClockAgeMs}ms threshold=${staleThreshold}ms after decode',
          );
          _rendering = false;
          _drainPending();
          return;
        }
        final previous = imageListenable.value;
        imageListenable.value = image;
        frameSizeListenable.value = Size(frame.width.toDouble(), frame.height.toDouble());
        previous?.dispose();
        if (frame.frameId % 30 == 0) {
          onLog(
            'Render submitted: frame ${frame.frameId} queueAge=${renderedAgeMs}ms hostClockAge=${frame.hostClockAgeMs}ms renderWait=${DateTime.now().millisecondsSinceEpoch - renderStartMs}ms',
          );
        }
        onPresented(frame, renderedAgeMs);
        _rendering = false;
        _drainPending();
      },
      rowBytes: frame.width * 4,
    );
  }

  void _drainPending() {
    final nextFrame = _pendingFrame;
    _pendingFrame = null;
    if (nextFrame != null) {
      submit(nextFrame);
    }
  }
}

class RemoteHomePage extends StatefulWidget {
  const RemoteHomePage({super.key});

  @override
  State<RemoteHomePage> createState() => _RemoteHomePageState();
}

class _RemoteHomePageState extends State<RemoteHomePage> {
  static const int _maxTransferBytes = 100 * 1024 * 1024;
  static const List<int> _reconnectBackoffSeconds = <int>[1, 2, 5, 10];
  static const double _tapSlopSquared = 400;
  static const Duration _directHealthTimeout = Duration(seconds: 20);
  static const Duration _relayHealthTimeout = Duration(seconds: 120);
  static const Duration _startupHealthTimeout = Duration(seconds: 15);
  static const Duration _healthPollInterval = Duration(seconds: 2);
  static const Duration _resyncCooldown = Duration(seconds: 15);
  static const Duration _resyncRecoveryTimeout = Duration(seconds: 15);
  static const String _savedHostsPrefsKey = 'saved_hosts_v1';
  static const String _lastHostPrefsKey = 'last_host';
  static const String _lastPortPrefsKey = 'last_port';
  static const String _lastTokenPrefsKey = 'last_token';
  static const String _lastUseRelayModePrefsKey = 'last_use_relay_mode';
  static const String _lastRelayHostIdPrefsKey = 'last_relay_host_id';
  static const String _lastRelayTokenPrefsKey = 'last_relay_token';
  static const String _lastResolutionPrefsKey = 'last_resolution';
  static const String _lastFpsPrefsKey = 'last_fps';
  static const String _lastJpegQualityPrefsKey = 'last_jpeg_quality';
  static const String _lastViewOnlyPrefsKey = 'last_view_only';
  static const String _lastMonitorIndexPrefsKey = 'last_monitor_index';
  static const String _lastAutoReconnectPrefsKey = 'last_auto_reconnect';
  static const String _lastClipboardModePrefsKey = 'last_clipboard_mode';
  static const String _lastDeltaStreamPrefsKey = 'last_delta_stream';
  static const String _lastAudioEnabledPrefsKey = 'last_audio_enabled';
  static const String _trustStatePrefsKey = 'trust_state';
  static const String _trustDeviceIdPrefsKey = 'trust_device_id';
  static const String _trustDeviceNamePrefsKey = 'trust_device_name';

  final TextEditingController _hostController = TextEditingController(text: '10.0.2.2');
  final TextEditingController _portController = TextEditingController(text: '6000');
  final TextEditingController _tokenController = TextEditingController();
  final TextEditingController _relayHostIdController = TextEditingController(text: 'default-host');
  final TextEditingController _relayTokenController = TextEditingController();

  final _settings = _SettingsState();

  RemoteClient? _remoteClient;
  StreamSubscription<RemoteVideoFrame>? _frameSubscription;
  StreamSubscription<Map<String, dynamic>>? _controlSubscription;
  Timer? _healthTimer;
  late final _LatestFramePresenter _framePresenter;

  List<SavedHostEntry> _savedHosts = const [];
  String? _selectedSavedHostLabel;
  String? _message;
  String? _lastError;
  bool _connecting = false;
  bool _reconnecting = false;
  bool _connected = false;
  bool _loadingSavedHosts = true;
  bool _localViewOnly = false;
  bool _transferCancelled = false;
  bool _autoReconnectEnabled = true;
  bool _useRelayMode = false;
  Offset? _pointerDownPosition;
  bool _pointerMovedSinceDown = false;
  double? _transferProgress;
  String? _transferStatus;
  String? _hostClipboardText;
  String? _trustedDeviceId;
  String _trustedDeviceName = 'Android Device';
  TrustState _trustState = TrustState.unpaired;
  int _reconnectAttempt = 0;
  int _streamKeyframesSent = 0;
  int _streamDeltaFramesSent = 0;
  int _streamResyncRequests = 0;
  int _streamInferredMoveFrames = 0;
  int _streamLastPatchCount = 0;
  int _streamLastMoveCount = 0;
  double _streamLastChangedRatio = 0;
  bool _manualDisconnectRequested = false;
  List<int> _availableMonitorIndexes = const <int>[0, 1, 2, 3];
  Map<int, String> _monitorLabels = const <int, String>{};
  List<String> _clientLogs = <String>[];
  ConnectionStage _connectionStage = ConnectionStage.idle;
  DateTime? _lastFrameAt;
  bool _hasRenderedFrame = false;
  int _lastRenderedFrameAgeMs = 0;
  String? _lastViewportSignature;
  StreamPhase _streamPhase = StreamPhase.connecting;
  DateTime? _connectStartedAt;
  DateTime? _firstFrameReceivedAt;
  bool _awaitingResyncKeyframe = false;
  DateTime? _lastSessionActivityAt;
  DateTime? _lastResyncRequestAt;

  @override
  void initState() {
    super.initState();
    _framePresenter = _LatestFramePresenter(
      onLog: _recordLog,
      onPresented: _handlePresentedFrame,
      isAwaitingFirstFrame: () => _streamPhase != StreamPhase.streaming,
    );
    unawaited(_loadSavedHosts());
    unawaited(_loadClientPrefs());
    unawaited(_bootstrapTrustIdentity());
  }

  @override
  void dispose() {
    _healthTimer?.cancel();
    _frameSubscription?.cancel();
    _controlSubscription?.cancel();
    _framePresenter.dispose();
    _remoteClient?.close();
    _hostController.dispose();
    _portController.dispose();
    _tokenController.dispose();
    _relayHostIdController.dispose();
    _relayTokenController.dispose();
    super.dispose();
  }

  void _handlePresentedFrame(RemoteVideoFrame frame, int renderedAgeMs) {
    if (!mounted) {
      return;
    }
    final firstPresentedFrame = !_hasRenderedFrame;
    final now = DateTime.now();
    _markSessionActivity('frame rendered', at: now);
    _hasRenderedFrame = true;
    _connected = true;
    _connectionStage = ConnectionStage.connected;
    _reconnectAttempt = 0;
    _lastRenderedFrameAgeMs = renderedAgeMs;
    _awaitingResyncKeyframe = false;
    if (firstPresentedFrame) {
      _streamPhase = StreamPhase.streaming;
      final connectStartedAt = _connectStartedAt;
      if (connectStartedAt != null) {
        _recordLog(
          'First frame rendered successfully: frame ${frame.frameId}, time_to_first_render_ms=${now.difference(connectStartedAt).inMilliseconds}, queue_age=${renderedAgeMs}ms',
        );
      } else {
        _recordLog('First frame rendered successfully: frame ${frame.frameId}, queue_age=${renderedAgeMs}ms');
      }
      _recordLog('Session phase changed to streaming');
    }
    _message = _useRelayMode
        ? 'Connected via relay to ${_relayHostIdController.text.trim()}'
        : 'Connected to ${_hostController.text.trim()}:${int.tryParse(_portController.text) ?? 0}';
    if (firstPresentedFrame || frame.frameId % 10 == 0) {
      setState(() {});
    }
  }

  Future<void> _loadSavedHosts() async {
    final prefs = await SharedPreferences.getInstance();
    final raw = prefs.getString(_savedHostsPrefsKey);
    if (raw == null || raw.isEmpty) {
      if (mounted) {
        setState(() {
          _loadingSavedHosts = false;
        });
      }
      return;
    }

    try {
      final decoded = jsonDecode(raw) as List<dynamic>;
      final entries = decoded
          .map((item) => SavedHostEntry.fromJson((item as Map).cast<String, dynamic>()))
          .where((entry) => entry.host.isNotEmpty)
          .toList();
      final deduped = <SavedHostEntry>[];
      final seenLabels = <String>{};
      for (final entry in entries) {
        if (seenLabels.add(entry.label)) {
          deduped.add(entry);
        }
      }

      if (!mounted) {
        return;
      }

      setState(() {
        _savedHosts = deduped;
        if (_selectedSavedHostLabel != null &&
            _savedHosts.where((entry) => entry.label == _selectedSavedHostLabel).length != 1) {
          _selectedSavedHostLabel = null;
        }
        _loadingSavedHosts = false;
      });

      if (deduped.isNotEmpty) {
        _applySavedHost(deduped.first);
      }
    } catch (_) {
      if (mounted) {
        setState(() {
          _loadingSavedHosts = false;
          _message = 'Saved hosts could not be loaded.';
        });
      }
    }
  }

  Future<void> _persistSavedHosts() async {
    final prefs = await SharedPreferences.getInstance();
    final deduped = <SavedHostEntry>[];
    final seenLabels = <String>{};
    for (final entry in _savedHosts) {
      if (seenLabels.add(entry.label)) {
        deduped.add(entry);
      }
    }
    final raw = jsonEncode(deduped.map((entry) => entry.toJson()).toList());
    await prefs.setString(_savedHostsPrefsKey, raw);
  }

  Future<void> _loadClientPrefs() async {
    final prefs = await SharedPreferences.getInstance();
    final host = prefs.getString(_lastHostPrefsKey);
    final port = prefs.getString(_lastPortPrefsKey);
    final token = prefs.getString(_lastTokenPrefsKey);
    final useRelayMode = prefs.getBool(_lastUseRelayModePrefsKey);
    final relayHostId = prefs.getString(_lastRelayHostIdPrefsKey);
    final relayToken = prefs.getString(_lastRelayTokenPrefsKey);
    final resolution = prefs.getString(_lastResolutionPrefsKey);
    final fps = prefs.getInt(_lastFpsPrefsKey);
    final jpegQuality = prefs.getDouble(_lastJpegQualityPrefsKey);
    final viewOnly = prefs.getBool(_lastViewOnlyPrefsKey);
    final monitorIndex = prefs.getInt(_lastMonitorIndexPrefsKey);
    final autoReconnect = prefs.getBool(_lastAutoReconnectPrefsKey);
    final clipboardMode = prefs.getString(_lastClipboardModePrefsKey);
    final deltaStreamEnabled = prefs.getBool(_lastDeltaStreamPrefsKey);
    final audioEnabled = prefs.getBool(_lastAudioEnabledPrefsKey);
    final trustState = prefs.getString(_trustStatePrefsKey);
    final trustDeviceId = prefs.getString(_trustDeviceIdPrefsKey);
    final trustDeviceName = prefs.getString(_trustDeviceNamePrefsKey);

    if (!mounted) {
      return;
    }

    setState(() {
      if (host != null && host.isNotEmpty) {
        _hostController.text = host;
      }
      if (port != null && port.isNotEmpty) {
        _portController.text = port;
      }
      if (token != null) {
        _tokenController.text = token;
      }
      if (useRelayMode != null) {
        _useRelayMode = useRelayMode;
      }
      if (relayHostId != null) {
        _relayHostIdController.text = relayHostId;
      }
      if (relayToken != null) {
        _relayTokenController.text = relayToken;
      }
      if (resolution != null) {
        _settings.resolution = ResolutionPreset.values.firstWhere(
          (value) => value.name == resolution,
          orElse: () => _settings.resolution,
        );
      }
      if (fps != null) {
        _settings.fps = fps;
      }
      if (jpegQuality != null) {
        _settings.jpegQuality = jpegQuality;
      }
      if (viewOnly != null) {
        _settings.viewOnly = viewOnly;
      }
      if (monitorIndex != null) {
        _settings.monitorIndex = monitorIndex;
      }
      if (autoReconnect != null) {
        _autoReconnectEnabled = autoReconnect;
      }
      if (clipboardMode != null) {
        _settings.clipboardMode = ClipboardSyncMode.values.firstWhere(
          (value) => value == _clipboardModeFromWire(clipboardMode),
          orElse: () => _settings.clipboardMode,
        );
      }
      if (deltaStreamEnabled != null) {
        _settings.deltaStreamEnabled = deltaStreamEnabled;
      }
      if (audioEnabled != null) {
        _settings.audioEnabled = audioEnabled;
        if (!audioEnabled) {
          unawaited(_remoteClient?.stopAudioOutput() ?? Future<void>.value());
        }
      }
      if (trustState != null) {
        _trustState = TrustState.values.firstWhere(
          (value) => value.name == trustState,
          orElse: () => TrustState.unpaired,
        );
      }
      if (trustDeviceId != null && trustDeviceId.isNotEmpty) {
        _trustedDeviceId = trustDeviceId;
      }
      if (trustDeviceName != null && trustDeviceName.isNotEmpty) {
        _trustedDeviceName = trustDeviceName;
      }
    });
  }

  Future<void> _persistClientPrefs() async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_lastHostPrefsKey, _hostController.text.trim());
    await prefs.setString(_lastPortPrefsKey, _portController.text.trim());
    await prefs.setString(_lastTokenPrefsKey, _tokenController.text);
    await prefs.setBool(_lastUseRelayModePrefsKey, _useRelayMode);
    await prefs.setString(_lastRelayHostIdPrefsKey, _relayHostIdController.text.trim());
    await prefs.setString(_lastRelayTokenPrefsKey, _relayTokenController.text);
    await prefs.setString(_lastResolutionPrefsKey, _settings.resolution.name);
    await prefs.setInt(_lastFpsPrefsKey, _settings.fps);
    await prefs.setDouble(_lastJpegQualityPrefsKey, _settings.jpegQuality);
    await prefs.setBool(_lastViewOnlyPrefsKey, _settings.viewOnly);
    await prefs.setInt(_lastMonitorIndexPrefsKey, _settings.monitorIndex);
    await prefs.setBool(_lastAutoReconnectPrefsKey, _autoReconnectEnabled);
    await prefs.setString(_lastClipboardModePrefsKey, _clipboardModeToWire(_settings.clipboardMode));
    await prefs.setBool(_lastDeltaStreamPrefsKey, _settings.deltaStreamEnabled);
    await prefs.setBool(_lastAudioEnabledPrefsKey, _settings.audioEnabled);
    await prefs.setString(_trustStatePrefsKey, _trustState.name);
    if (_trustedDeviceId != null && _trustedDeviceId!.isNotEmpty) {
      await prefs.setString(_trustDeviceIdPrefsKey, _trustedDeviceId!);
    } else {
      await prefs.remove(_trustDeviceIdPrefsKey);
    }
    await prefs.setString(_trustDeviceNamePrefsKey, _trustedDeviceName);
  }

  void _applySavedHost(SavedHostEntry entry) {
    _hostController.text = entry.host;
    _portController.text = entry.port.toString();
    _tokenController.text = entry.token;
    _relayHostIdController.text = entry.relayHostId;
    _relayTokenController.text = entry.relayToken;
    setState(() {
      _selectedSavedHostLabel = entry.label;
      _useRelayMode = entry.useRelayMode;
      _settings.resolution = entry.resolution;
      _settings.fps = entry.fps;
      _settings.jpegQuality = entry.jpegQuality;
      _settings.viewOnly = entry.viewOnly;
      _settings.monitorIndex = entry.monitorIndex;
      _autoReconnectEnabled = entry.autoReconnect;
      _settings.clipboardMode = entry.clipboardMode;
      _settings.deltaStreamEnabled = entry.deltaStreamEnabled;
      _settings.audioEnabled = entry.audioEnabled;
      _message = 'Loaded host "${entry.label}".';
      _recordLog('Loaded saved host ${entry.label} -> ${entry.host}:${entry.port}');
    });
    unawaited(_persistClientPrefs());
  }

  Future<void> _bootstrapTrustIdentity() async {
    try {
      final identity = await AndroidTrustIdentityService.instance.getOrCreateDeviceIdentity();
      if (!mounted) {
        return;
      }
      setState(() {
        _trustedDeviceId = identity.deviceId;
        if (identity.deviceName.trim().isNotEmpty) {
          _trustedDeviceName = identity.deviceName.trim();
        }
      });
      await _persistClientPrefs();
    } catch (e) {
      if (!mounted) {
        return;
      }
      setState(() {
        _recordLog('Trust identity bootstrap failed: $e');
      });
    }
  }

  Future<void> _pairThisDevice({bool forceRePair = false}) async {
    final client = _remoteClient;
    if (client == null || !_connected) {
      setState(() {
        _message = 'Connect first before pairing.';
      });
      _showSnack('Connect first before pairing');
      return;
    }

    try {
      final identity = await client.getOrCreateDeviceIdentity();
      setState(() {
        _trustedDeviceId = identity.deviceId;
        _trustedDeviceName = identity.deviceName;
        _trustState = TrustState.pending;
        _recordLog('Pair request started for ${identity.deviceName} (${identity.deviceId})');
      });
      await _persistClientPrefs();
      await client.requestPairing(deviceName: identity.deviceName);
      if (!mounted) {
        return;
      }
      setState(() {
        _message = forceRePair ? 'Re-pair request sent. Waiting for host approval.' : 'Pair request sent. Waiting for host approval.';
      });
      _showSnack('Pair request sent');
    } catch (e) {
      if (!mounted) {
        return;
      }
      setState(() {
        _message = 'Pair request failed: $e';
        _recordLog('Pair request failed: $e');
        if (_trustState == TrustState.pending) {
          _trustState = TrustState.unpaired;
        }
      });
      _showSnack('Pair request failed');
    }
  }

  Future<void> _forgetLocalTrustMetadata() async {
    try {
      await _remoteClient?.forgetLocalIdentity();
      await AndroidTrustIdentityService.instance.forgetLocalIdentity();
      if (!mounted) {
        return;
      }
      setState(() {
        _trustedDeviceId = null;
        _trustedDeviceName = 'Android Device';
        _trustState = TrustState.unpaired;
        _message = 'Local trust identity cleared.';
        _recordLog('Local trust identity cleared');
      });
      await _persistClientPrefs();
      _showSnack('Local trust metadata cleared');
      unawaited(_bootstrapTrustIdentity());
    } catch (e) {
      if (!mounted) {
        return;
      }
      setState(() {
        _message = 'Failed to clear trust metadata: $e';
        _recordLog('Failed to clear trust metadata: $e');
      });
      _showSnack('Failed to clear trust metadata');
    }
  }

  Future<void> _saveCurrentHost({bool updateExisting = false}) async {
    try {
      final host = _hostController.text.trim();
      final port = int.tryParse(_portController.text.trim());
      final token = _tokenController.text;
      if (host.isEmpty || port == null || port <= 0 || port > 65535) {
        setState(() {
          _message = 'Enter a valid host and port before saving.';
          _recordLog('Save blocked: invalid host=$host port=${_portController.text.trim()}');
        });
        return;
      }

      final label = updateExisting && _selectedSavedHostLabel != null
          ? _selectedSavedHostLabel!
          : '$host:$port${_useRelayMode ? ' [relay]' : ''}';

      final entry = SavedHostEntry(
        label: label,
        host: host,
        port: port,
        token: token,
        useRelayMode: _useRelayMode,
        relayHostId: _relayHostIdController.text.trim(),
        relayToken: _relayTokenController.text,
        resolution: _settings.resolution,
        fps: _settings.fps,
        jpegQuality: _settings.jpegQuality,
        viewOnly: _settings.viewOnly,
        monitorIndex: _settings.monitorIndex,
        autoReconnect: _autoReconnectEnabled,
        clipboardMode: _settings.clipboardMode,
        deltaStreamEnabled: _settings.deltaStreamEnabled,
        audioEnabled: _settings.audioEnabled,
      );

      final next = _savedHosts.where((item) {
        if (item.label == label) {
          return false;
        }
        if (updateExisting && _selectedSavedHostLabel != null && item.label == _selectedSavedHostLabel) {
          return false;
        }
        return true;
      }).toList();
      next.add(entry);

      setState(() {
        _savedHosts = next;
        _selectedSavedHostLabel = entry.label;
        _message = 'Saved host "${entry.label}".';
        _recordLog(
          'Saved host ${entry.label} -> ${entry.host}:${entry.port} relay=${entry.useRelayMode} relayHostId=${entry.relayHostId}',
        );
      });
      await _persistSavedHosts();
    } catch (e, st) {
      debugPrint('saveCurrentHost failed: $e');
      debugPrint('$st');
      if (!mounted) {
        return;
      }
      setState(() {
        _lastError = e.toString();
        _message = 'Save failed: $e';
        _recordLog('Save failed: $e');
      });
      _showSnack('Save failed: $e');
    }
  }

  Future<void> _deleteSelectedHost() async {
    final label = _selectedSavedHostLabel;
    if (label == null) {
      setState(() {
        _message = 'Select a saved host first.';
      });
      return;
    }

    final next = _savedHosts.where((entry) => entry.label != label).toList();
    setState(() {
      _savedHosts = next;
      _selectedSavedHostLabel = null;
      _message = 'Deleted saved host "$label".';
      _recordLog('Deleted saved host $label');
    });
    await _persistSavedHosts();
  }

  Future<void> _connectAndStream({bool isReconnect = false}) async {
    final host = _hostController.text.trim();
    final port = int.tryParse(_portController.text.trim());
    final token = _tokenController.text;
    final relayHostId = _relayHostIdController.text.trim();
    final relayToken = _relayTokenController.text.trim();

    if (host.isEmpty || port == null || port <= 0 || port > 65535) {
      setState(() {
        _message = 'Enter a valid host and port.';
      });
      return;
    }
    if (_useRelayMode && relayHostId.isEmpty) {
      setState(() {
        _message = 'Enter a relay host ID.';
      });
      return;
    }

    setState(() {
      _manualDisconnectRequested = false;
      _streamPhase = StreamPhase.connecting;
      _connectStartedAt = DateTime.now();
      _firstFrameReceivedAt = null;
      _awaitingResyncKeyframe = false;
      _connecting = true;
      _message = isReconnect
          ? 'Reconnecting to $host:$port...'
          : (_useRelayMode
              ? 'Connecting to relay $host:$port for host $relayHostId...'
              : 'Connecting to $host:$port...');
      _lastError = null;
      _connectionStage = isReconnect ? ConnectionStage.reconnecting : ConnectionStage.connecting;
      _recordLog(
        '${isReconnect ? 'Reconnect' : 'Connect'} requested for $host:$port'
        '${_useRelayMode ? ' (relay host_id=$relayHostId)' : ''}',
      );
      _recordLog('Session phase changed to connecting');
    });
    unawaited(_persistClientPrefs());

    try {
      await _disconnect();
      _framePresenter.clear();

      final client = RemoteClient(
        host: host,
        port: port,
        authToken: token,
        relayHostId: _useRelayMode ? relayHostId : null,
        relayToken: _useRelayMode && relayToken.isNotEmpty ? relayToken : null,
      );
      client.updateClipboardMode(_clipboardModeToWire(_settings.clipboardMode));
      client.setTrustedAuthEnabled(_trustState == TrustState.trusted);
      setState(() {
        _connectionStage = ConnectionStage.tlsHandshake;
      });
      await client.connect();
      setState(() {
        _connectionStage = ConnectionStage.authenticating;
        _streamPhase = StreamPhase.awaitingFirstFrame;
      });
      _recordLog('Session phase changed to awaiting_first_frame');
      await _applySettings(client);
      final controlSub = client.controlMessages.listen(_handleControlMessage);

      final sub = client.streamFrames().listen(
        (frame) {
          if (!mounted) {
            return;
          }

          final now = DateTime.now();
          _lastFrameAt = now;
          _markSessionActivity('frame received', at: now);
          _firstFrameReceivedAt ??= now;
          if (_firstFrameReceivedAt == now) {
            final connectStartedAt = _connectStartedAt;
            _recordLog(
              'First frame received: frame ${frame.frameId}, queue_age=${frame.queueAgeMs}ms, host_clock_age=${frame.hostClockAgeMs}ms${connectStartedAt == null ? '' : ', time_to_first_frame_ms=${now.difference(connectStartedAt).inMilliseconds}'}',
            );
          }
          _framePresenter.submit(frame);
        },
        onError: (err) async {
          if (!mounted) {
            return;
          }
          setState(() {
            _message = 'Stream error: $err';
            _lastError = err.toString();
            _connected = false;
            _hasRenderedFrame = false;
            _streamPhase = StreamPhase.connecting;
            _connectionStage = _mapErrorToConnectionStage(err);
            _recordLog('Stream error: $err');
          });
          _framePresenter.clear();
          await _disconnect();
          _queueReconnectIfAllowed('stream error');
        },
        onDone: () async {
          if (!mounted) {
            return;
          }
          setState(() {
            _message = _manualDisconnectRequested
                ? 'Disconnected.'
                : 'Stream ended. Tap Connect to retry.';
            _connected = false;
            _hasRenderedFrame = false;
            _streamPhase = StreamPhase.connecting;
            _connectionStage =
                _manualDisconnectRequested ? ConnectionStage.stopped : ConnectionStage.disconnected;
            _recordLog(_manualDisconnectRequested ? 'Disconnected by user' : 'Stream ended');
          });
          _framePresenter.clear();
          await _disconnect();
          _queueReconnectIfAllowed('stream ended');
        },
        cancelOnError: true,
      );

      _remoteClient = client;
      _frameSubscription = sub;
      _controlSubscription = controlSub;
      _startHealthChecks();

      setState(() {
        _connected = true;
      });
    } catch (e) {
      setState(() {
        _message = 'Connect failed: $e';
        _lastError = e.toString();
        _connected = false;
        _hasRenderedFrame = false;
        _streamPhase = StreamPhase.connecting;
        _connectionStage = _mapErrorToConnectionStage(e);
        _recordLog('Connect failed: $e');
      });
      _framePresenter.clear();
      _queueReconnectIfAllowed('connect failure');
    } finally {
      if (mounted) {
        setState(() {
          _connecting = false;
        });
      } else {
        _connecting = false;
      }
    }
  }

  Future<void> _disconnect({bool manual = false}) async {
    if (manual) {
      _manualDisconnectRequested = true;
    }
    _healthTimer?.cancel();
    _healthTimer = null;
    await _frameSubscription?.cancel();
    _frameSubscription = null;
    await _controlSubscription?.cancel();
    _controlSubscription = null;
    await _remoteClient?.close();
    _remoteClient = null;
    _framePresenter.clear();

    if (mounted) {
      setState(() {
        _connected = false;
        _hasRenderedFrame = false;
        _streamPhase = StreamPhase.connecting;
        _awaitingResyncKeyframe = false;
        if (!_connecting && !_reconnecting) {
          _connectionStage = manual ? ConnectionStage.stopped : ConnectionStage.disconnected;
        }
        if (manual) {
          _recordLog('Disconnect requested by user');
        }
      });
    }
  }

  void _markSessionActivity(String reason, {DateTime? at}) {
    final timestamp = at ?? DateTime.now();
    final previous = _lastSessionActivityAt;
    _lastSessionActivityAt = timestamp;
    if (previous == null || timestamp.difference(previous) >= const Duration(seconds: 5)) {
      _recordLog('Health activity: $reason');
    }
  }

  void _startHealthChecks() {
    _healthTimer?.cancel();
    _recordLog(
      'Health timer started (${_useRelayMode ? 'relay' : 'direct'}) startup_threshold=${_startupHealthTimeout.inSeconds}s steady_threshold=${(_useRelayMode ? _relayHealthTimeout : _directHealthTimeout).inSeconds}s',
    );
    _healthTimer = Timer.periodic(_healthPollInterval, (_) async {
      final lastActivityAt = _lastSessionActivityAt ?? _lastFrameAt;
      if (!_connected) {
        return;
      }

      final now = DateTime.now();
      final threshold = _hasRenderedFrame
          ? (_useRelayMode ? _relayHealthTimeout : _directHealthTimeout)
          : _startupHealthTimeout;
      final baselineAt = lastActivityAt ?? _connectStartedAt;
      if (baselineAt == null) {
        return;
      }

      final idleFor = now.difference(baselineAt);
      final stale = idleFor > threshold;
      if (!stale) {
        return;
      }

      if (!_hasRenderedFrame) {
        _recordLog(
          'Startup health warning: no first frame yet after ${idleFor.inSeconds}s (threshold=${threshold.inSeconds}s)',
        );
      }

      if (_awaitingResyncKeyframe) {
        if (idleFor > threshold + _resyncRecoveryTimeout) {
          _recordLog(
            'Health recovery failed: no keyframe after resync for ${idleFor.inSeconds}s, reconnecting',
          );
          await _disconnect();
          _queueReconnectIfAllowed('resync recovery timeout');
          return;
        }
        _recordLog(
          'Health timeout suppressed: already awaiting resync keyframe for ${idleFor.inSeconds}s',
        );
        return;
      }

      final lastResyncRequestAt = _lastResyncRequestAt;
      final shouldRequestResync =
          lastResyncRequestAt == null ||
          now.difference(lastResyncRequestAt) > _resyncCooldown;

      if (shouldRequestResync) {
        _recordLog(
          'Health timeout fired after ${idleFor.inSeconds}s (threshold=${threshold.inSeconds}s), requesting resync (phase=${_streamPhase.name})',
        );
        try {
          await _requestResyncWithGuard('health timeout after ${idleFor.inSeconds}s');
        } catch (err) {
          if (!mounted) {
            return;
          }
          setState(() {
            _message = 'Health check failed: $err';
            _lastError = err.toString();
            _connectionStage = ConnectionStage.reconnecting;
            _recordLog('Health check resync failed: $err');
          });
          await _disconnect();
          _queueReconnectIfAllowed('health check timeout');
        }
      }
    });
  }

  void _queueReconnectIfAllowed(String reason) {
    if (!_autoReconnectEnabled ||
        _manualDisconnectRequested ||
        _connectionStage == ConnectionStage.authFailed ||
        _connectionStage == ConnectionStage.tlsFailed) {
      return;
    }
    unawaited(Future<void>.delayed(Duration.zero, () => _scheduleReconnect(reason: reason)));
  }

  Future<void> _scheduleReconnect({required String reason}) async {
    if (_connecting || _reconnecting) {
      return;
    }
    if (_reconnectAttempt >= _reconnectBackoffSeconds.length) {
      if (mounted) {
        setState(() {
          _message = 'Auto reconnect stopped after ${_reconnectBackoffSeconds.length} attempts.';
          _connectionStage = ConnectionStage.disconnected;
        });
      }
      return;
    }

    final delaySeconds = _reconnectBackoffSeconds[_reconnectAttempt];
    _reconnectAttempt += 1;
    _reconnecting = true;
    if (mounted) {
      setState(() {
        _connectionStage = ConnectionStage.reconnecting;
        _message =
            'Reconnect attempt $_reconnectAttempt/${_reconnectBackoffSeconds.length} in ${delaySeconds}s ($reason)';
        _recordLog('Reconnect attempt $_reconnectAttempt scheduled in ${delaySeconds}s ($reason)');
      });
    }
    await _disconnect();
    await Future<void>.delayed(Duration(seconds: delaySeconds));
    if (mounted) {
      await _connectAndStream(isReconnect: true);
    }
    _reconnecting = false;
  }

  Future<void> _applySettings([RemoteClient? target]) async {
    final client = target ?? _remoteClient;
    if (client == null) {
      setState(() {
        _message = 'Connect first before applying settings.';
      });
      return;
    }

    try {
      await client.sendSettings(
        targetWidth: _settings.targetWidth,
        fps: _settings.fps,
        jpegQuality: _settings.jpegQuality.round(),
        viewOnly: _settings.viewOnly,
        monitorIndex: _settings.monitorIndex,
        clipboardMode: _clipboardModeToWire(_settings.clipboardMode),
        deltaStreamEnabled: _settings.deltaStreamEnabled,
        audioEnabled: _settings.audioEnabled,
      );
      if (mounted) {
        setState(() {
          _message = 'Settings applied.';
          _recordLog(
            'Settings applied: monitor ${_settings.monitorIndex}, ${_settings.targetWidth}px, ${_settings.fps} FPS, JPEG ${_settings.jpegQuality.round()}, view-only=${_settings.viewOnly}, clipboard=manual, delta=${_settings.deltaStreamEnabled}, audio=${_settings.audioEnabled}',
          );
        });
      }
      unawaited(_persistClientPrefs());
    } catch (e) {
      if (mounted) {
        setState(() {
          _message = 'Failed to apply settings: $e';
        });
      }
    }
  }

  void _showSnack(String message) {
    if (!mounted) {
      return;
    }
    ScaffoldMessenger.of(context)
      ..hideCurrentSnackBar()
      ..showSnackBar(SnackBar(content: Text(message)));
  }

  Future<void> _sendClipboardToHost() async {
    final client = _remoteClient;
    if (client == null) {
      setState(() {
        _message = 'Connect first.';
      });
      _showSnack('Clipboard sync failed: not connected');
      return;
    }

    final text = await client.getLocalClipboard() ?? '';
    if (text.isEmpty) {
      setState(() {
        _message = 'Clipboard is empty.';
      });
      _showSnack('Clipboard empty');
      return;
    }

    try {
      await client.sendClipboardText(text);
      setState(() {
        _message = 'Clipboard sent to host.';
        _recordLog('Clipboard sent to host (${text.length} chars)');
      });
      _showSnack('Clipboard sent to PC');
    } catch (e) {
      setState(() {
        _message = 'Clipboard send failed: $e';
      });
      _showSnack('Clipboard sync failed');
    }
  }

  Future<void> _requestClipboardFromHost() async {
    final client = _remoteClient;
    if (client == null) {
      setState(() {
        _message = 'Connect first.';
      });
      _showSnack('Clipboard sync failed: not connected');
      return;
    }

    try {
      await client.requestClipboardText();
      setState(() {
        _message = 'Requested clipboard from host.';
        _recordLog('Requested clipboard from host');
      });
    } catch (e) {
      setState(() {
        _message = 'Clipboard request failed: $e';
      });
      _showSnack('Clipboard sync failed');
    }
  }

  Future<void> _pickAndSendFile() async {
    final client = _remoteClient;
    if (client == null) {
      setState(() {
        _message = 'Connect first.';
      });
      return;
    }

    final result = await FilePicker.platform.pickFiles(withData: true);
    final file = result?.files.singleOrNull;
    if (file == null || file.bytes == null) {
      setState(() {
        _message = 'File selection cancelled.';
      });
      return;
    }
    if (file.bytes!.length > _maxTransferBytes) {
      setState(() {
        _message = 'File too large. Max size is ${(_maxTransferBytes / (1024 * 1024)).round()} MB.';
        _transferStatus = 'Transfer blocked';
        _transferProgress = null;
      });
      return;
    }

    try {
      setState(() {
        _transferCancelled = false;
        _transferProgress = 0;
        _transferStatus = 'Sending ${file.name}...';
        _recordLog('File transfer started: ${file.name} (${file.bytes!.length} bytes)');
      });
      await client.sendFileBytes(
        file.name,
        file.bytes!,
        onProgress: (progress) {
          if (!mounted) {
            return;
          }
          setState(() {
            _transferProgress = progress;
            _transferStatus = 'Sending ${file.name} (${(progress * 100).round()}%)';
          });
        },
        shouldCancel: () => _transferCancelled,
      );
      setState(() {
        _message = 'Sent file "${file.name}". Waiting for host verification...';
        _recordLog('File transfer upload completed locally for ${file.name}');
      });
    } catch (e) {
      setState(() {
        _message = 'File send failed: $e';
        _transferStatus = 'Transfer failed';
        _lastError = e.toString();
        _recordLog('File transfer failed: $e');
      });
    }
  }

  void _cancelTransfer() {
    unawaited(_remoteClient?.sendFileCancel() ?? Future<void>.value());
    setState(() {
      _transferCancelled = true;
      _transferStatus = 'Cancelling transfer...';
      _recordLog('File transfer cancellation requested');
    });
  }

  Future<void> _handleLongPressStart(LongPressStartDetails details, BoxConstraints constraints) async {
    final client = _remoteClient;
    if (client == null || _localViewOnly || _settings.viewOnly) {
      return;
    }

    final relX = (details.localPosition.dx / constraints.maxWidth).clamp(0.0, 1.0);
    final relY = (details.localPosition.dy / constraints.maxHeight).clamp(0.0, 1.0);
    unawaited(() async {
      try {
        await client.sendMouseMove(relX, relY);
        await client.sendRightClick();
      } catch (e) {
        debugPrint('right click send failed: $e');
      }
    }());
  }

  void _handlePointerDown(Offset localPosition, BoxConstraints constraints) {
    final client = _remoteClient;
    if (client == null || _localViewOnly || _settings.viewOnly) {
      return;
    }

    _pointerDownPosition = localPosition;
    _pointerMovedSinceDown = false;
    final relX = (localPosition.dx / constraints.maxWidth).clamp(0.0, 1.0);
    final relY = (localPosition.dy / constraints.maxHeight).clamp(0.0, 1.0);
    _recordLog(
      'touch down local=(${localPosition.dx.toStringAsFixed(1)}, ${localPosition.dy.toStringAsFixed(1)}) '
      'size=(${constraints.maxWidth.toStringAsFixed(1)}, ${constraints.maxHeight.toStringAsFixed(1)}) '
      'rel=(${relX.toStringAsFixed(3)}, ${relY.toStringAsFixed(3)})',
    );
    unawaited(() async {
      try {
        await client.sendMouseMove(relX, relY);
      } catch (e) {
        debugPrint('pointer down send failed: $e');
      }
    }());
  }

  void _handlePointerMove(Offset localPosition, BoxConstraints constraints) {
    final client = _remoteClient;
    if (client == null || _localViewOnly || _settings.viewOnly) {
      return;
    }

    final down = _pointerDownPosition;
    if (down != null && (localPosition - down).distanceSquared > _tapSlopSquared) {
      _pointerMovedSinceDown = true;
    }

    final relX = (localPosition.dx / constraints.maxWidth).clamp(0.0, 1.0);
    final relY = (localPosition.dy / constraints.maxHeight).clamp(0.0, 1.0);
    unawaited(() async {
      try {
        await client.sendMouseMove(relX, relY);
      } catch (e) {
        debugPrint('pointer move send failed: $e');
      }
    }());
  }

  void _handlePointerUp(Offset localPosition, BoxConstraints constraints) {
    final client = _remoteClient;
    if (client == null || _localViewOnly || _settings.viewOnly) {
      return;
    }

    final relX = (localPosition.dx / constraints.maxWidth).clamp(0.0, 1.0);
    final relY = (localPosition.dy / constraints.maxHeight).clamp(0.0, 1.0);
    final shouldClick = !_pointerMovedSinceDown;
    _recordLog(
      'touch up rel=(${relX.toStringAsFixed(3)}, ${relY.toStringAsFixed(3)}) click=$shouldClick',
    );
    _pointerDownPosition = null;
    _pointerMovedSinceDown = false;
    unawaited(() async {
      try {
        if (shouldClick) {
          _recordLog('sending left_click rel=(${relX.toStringAsFixed(3)}, ${relY.toStringAsFixed(3)})');
          await client.sendLeftClick();
        } else {
          await client.sendMouseMove(relX, relY);
        }
      } catch (e) {
        _recordLog('pointer up send failed: $e');
        debugPrint('pointer up send failed: $e');
      }
    }());
  }

  Future<void> _sendScroll(int delta) async {
    final client = _remoteClient;
    if (client == null || _localViewOnly || _settings.viewOnly) {
      return;
    }

    try {
      await client.sendMouseScroll(delta);
      setState(() {
        _recordLog('Scroll event sent: $delta');
      });
    } catch (e) {
      debugPrint('scroll send failed: $e');
    }
  }

  int? _charToVk(String ch) {
    if (ch.isEmpty) {
      return null;
    }
    final code = ch.codeUnitAt(0);
    if (code >= 0x30 && code <= 0x39) {
      return code;
    }
    if (code >= 0x61 && code <= 0x7A) {
      return code - 32;
    }
    if (code >= 0x41 && code <= 0x5A) {
      return code;
    }
    if (ch == ' ') {
      return 0x20;
    }
    return null;
  }

  Future<void> _sendTextAsKeys(String text) async {
    final client = _remoteClient;
    if (client == null) {
      setState(() {
        _message = 'Connect first before sending keyboard input.';
      });
      return;
    }
    if (_localViewOnly || _settings.viewOnly) {
      setState(() {
        _message = 'Input disabled (view-only).';
      });
      return;
    }

    for (final rune in text.runes) {
      final ch = String.fromCharCode(rune);
      final vk = _charToVk(ch);
      if (vk == null) {
        continue;
      }
      await client.sendKeyDown(vk);
      await client.sendKeyUp(vk);
    }
  }

  Future<void> _sendVirtualKeyTap(int vk) async {
    final client = _remoteClient;
    if (client == null) {
      return;
    }
    await client.sendKeyDown(vk);
    await client.sendKeyUp(vk);
  }

  Future<void> _sendShortcut(String shortcut) async {
    final client = _remoteClient;
    if (client == null) {
      return;
    }

    switch (shortcut) {
      case 'tab':
        await _sendVirtualKeyTap(0x09);
        return;
      case 'esc':
        await _sendVirtualKeyTap(0x1B);
        return;
      case 'left':
        await _sendVirtualKeyTap(0x25);
        return;
      case 'up':
        await _sendVirtualKeyTap(0x26);
        return;
      case 'right':
        await _sendVirtualKeyTap(0x27);
        return;
      case 'down':
        await _sendVirtualKeyTap(0x28);
        return;
      case 'ctrl+c':
        await client.sendKeyCombo(const <int>[0x11, 0x43]);
        return;
      case 'ctrl+v':
        await client.sendKeyCombo(const <int>[0x11, 0x56]);
        return;
      case 'ctrl+a':
        await client.sendKeyCombo(const <int>[0x11, 0x41]);
        return;
      case 'alt+tab':
        await client.sendKeyCombo(const <int>[0x12, 0x09]);
        return;
      default:
        return;
    }
  }

  Future<void> _openKeyboardDialog() async {
    final result = await Navigator.of(context).push<String>(
      MaterialPageRoute(
        builder: (_) => const _KeyboardInputPage(),
      ),
    );

    if (!mounted || result == null || result == 'cancel') {
      return;
    }

    final client = _remoteClient;
    if (client == null) {
      setState(() {
        _message = 'Connect first before sending keyboard input.';
      });
      return;
    }

    if (_localViewOnly || _settings.viewOnly) {
      setState(() {
        _message = 'Input disabled (view-only).';
      });
      return;
    }

    if (result == 'backspace') {
      await _sendVirtualKeyTap(0x08);
      return;
    }

    if (result == 'enter') {
      await _sendVirtualKeyTap(0x0D);
      return;
    }

    if (result == 'tab' ||
        result == 'esc' ||
        result == 'left' ||
        result == 'up' ||
        result == 'right' ||
        result == 'down' ||
        result == 'ctrl+c' ||
        result == 'ctrl+v' ||
        result == 'ctrl+a' ||
        result == 'alt+tab') {
      await _sendShortcut(result);
      return;
    }

    if (result.startsWith('send:')) {
      await _sendTextAsKeys(result.substring(5));
      return;
    }

    if (result.startsWith('submit:')) {
      await _sendTextAsKeys(result.substring(7));
      await client.sendKeyDown(0x0D);
      await client.sendKeyUp(0x0D);
    }
  }


  void _handleControlMessage(Map<String, dynamic> message) {
    if (!mounted) {
      return;
    }
    final type = message['type'];
    _markSessionActivity('control:$type');
    if (type == 'monitor_inventory') {
      final monitors = (message['monitors'] as List<dynamic>? ?? const <dynamic>[]);
      final indexes = <int>[];
      final labels = <int, String>{};
      for (final item in monitors) {
        final map = (item as Map).cast<String, dynamic>();
        final index = (map['index'] as num?)?.toInt();
        if (index == null) {
          continue;
        }
        indexes.add(index);
        labels[index] = (map['label'] as String?) ?? 'Monitor $index';
      }
      setState(() {
        _availableMonitorIndexes = indexes.isEmpty ? const <int>[0, 1, 2, 3] : indexes;
        _monitorLabels = labels;
        if (!_availableMonitorIndexes.contains(_settings.monitorIndex)) {
          _settings.monitorIndex = _availableMonitorIndexes.first;
        }
        _recordLog('Received monitor inventory: ${labels.values.join(' | ')}');
      });
      return;
    }

    if (type == 'session_status') {
      final monitorIndex = (message['monitor_index'] as num?)?.toInt();
      final targetWidth = (message['target_width'] as num?)?.toInt();
      final fps = (message['fps'] as num?)?.toInt();
      final jpegQuality = (message['jpeg_quality'] as num?)?.toDouble();
      final viewOnly = message['view_only'] == true;
      final clipboardMode = message['clipboard_mode'] as String?;
      final deltaStreamEnabled = message['delta_stream_enabled'] == true;
      final audioEnabled = message['audio_enabled'] == true;
      setState(() {
        if (monitorIndex != null) {
          _settings.monitorIndex = monitorIndex;
        }
        if (targetWidth != null) {
          _settings.resolution = switch (targetWidth) {
            <= 960 => ResolutionPreset.low,
            <= 1280 => ResolutionPreset.medium,
            _ => ResolutionPreset.high,
          };
        }
        if (fps != null) {
          _settings.fps = fps;
        }
        if (jpegQuality != null) {
          _settings.jpegQuality = jpegQuality;
        }
        _settings.viewOnly = viewOnly;
        if (clipboardMode != null) {
          _settings.clipboardMode = ClipboardSyncMode.values.firstWhere(
            (value) => value == _clipboardModeFromWire(clipboardMode),
            orElse: () => _settings.clipboardMode,
          );
        }
        _settings.deltaStreamEnabled = deltaStreamEnabled;
        _settings.audioEnabled = audioEnabled;
        if (!audioEnabled) {
          unawaited(_remoteClient?.stopAudioOutput() ?? Future<void>.value());
        }
        _recordLog(
          'Session status from host: monitor ${_settings.monitorIndex}, ${_settings.targetWidth}px, ${_settings.fps} FPS, JPEG ${_settings.jpegQuality.round()}, view-only=${_settings.viewOnly}, clipboard=manual, delta=${_settings.deltaStreamEnabled}, audio=${_settings.audioEnabled}',
        );
      });
      return;
    }

    if (type == 'stream_stats') {
      setState(() {
        _streamKeyframesSent = (message['keyframes_sent'] as num?)?.toInt() ?? _streamKeyframesSent;
        _streamDeltaFramesSent = (message['delta_frames_sent'] as num?)?.toInt() ?? _streamDeltaFramesSent;
        _streamResyncRequests = (message['resync_requests'] as num?)?.toInt() ?? _streamResyncRequests;
        _streamInferredMoveFrames = (message['inferred_move_frames'] as num?)?.toInt() ?? _streamInferredMoveFrames;
        _streamLastPatchCount = (message['last_patch_count'] as num?)?.toInt() ?? _streamLastPatchCount;
        _streamLastMoveCount = (message['last_move_count'] as num?)?.toInt() ?? _streamLastMoveCount;
        _streamLastChangedRatio = (message['last_changed_ratio'] as num?)?.toDouble() ?? _streamLastChangedRatio;
      });
      return;
    }

    if (type == 'client_video_packet') {
      _markSessionActivity('video_packet');
      return;
    }

    if (type == 'client_audio_packet') {
      _markSessionActivity('audio_packet');
      return;
    }

    if (type == 'client_resync_needed') {
      final reason = message['reason'] as String? ?? 'unknown';
      final frameId = message['frame_id'];
      _recordLog('Decode requested resync: frame=$frameId reason=$reason');
      unawaited(_requestResyncWithGuard('decode:$reason frame=$frameId'));
      return;
    }

    if (type == 'clipboard_data') {
      final text = message['text'] as String? ?? '';
      final syncId = message['sync_id'] as String?;
      final appliedLocally = message['applied_locally'] == true;
      if (appliedLocally) {
        unawaited(Clipboard.setData(ClipboardData(text: text)));
      }
      setState(() {
        _hostClipboardText = text;
        _message = text.isEmpty
            ? 'Host clipboard is empty.'
            : (appliedLocally ? 'Clipboard updated from PC.' : 'Clipboard received from PC.');
        _recordLog('Received host clipboard (${text.length} chars, sync=${syncId ?? 'none'}, auto=$appliedLocally)');
      });
      _showSnack(text.isEmpty
          ? 'Clipboard empty'
          : (appliedLocally ? 'Clipboard updated from PC' : 'Clipboard received from PC'));
      return;
    }

    if (type == 'pair_proof_sent') {
      setState(() {
        _message = 'Pair proof sent. Waiting for host decision.';
        _recordLog('Pair proof sent to host');
      });
      return;
    }

    if (type == 'pair_result') {
      final ok = message['ok'] == true;
      final resultMessage = message['message'] as String? ?? 'pair_result';
      final fingerprint = message['fingerprint'] as String?;
      _remoteClient?.setTrustedAuthEnabled(ok);
      setState(() {
        if (ok) {
          _trustState = TrustState.trusted;
          _message = fingerprint == null
              ? 'Device trusted successfully.'
              : 'Device trusted successfully. $fingerprint';
        } else if (resultMessage == 'waiting_for_host_approval') {
          _trustState = TrustState.pending;
          _message = 'Pair request sent. Waiting for host approval.';
        } else if (resultMessage == 'revoked') {
          _trustState = TrustState.revoked;
          _message = 'This device has been revoked by the host.';
        } else {
          _trustState = TrustState.rejected;
          _message = 'Pairing failed: $resultMessage';
        }
        _recordLog('Pair result: ok=$ok, message=$resultMessage');
      });
      unawaited(_persistClientPrefs());
      _showSnack(ok ? 'Trusted successfully' : (_trustState == TrustState.pending ? 'Waiting for host approval' : _message ?? 'Pairing failed'));
      return;
    }

    if (type == 'trusted_auth_sent') {
      setState(() {
        _recordLog('Trusted auth sent for device ${message['device_id'] ?? 'unknown'}');
      });
      return;
    }

    if (type == 'trusted_auth_result') {
      final ok = message['ok'] == true;
      final revoked = message['revoked'] == true;
      final resultMessage = message['message'] as String? ?? 'trusted_auth_result';
      _remoteClient?.setTrustedAuthEnabled(ok);
      setState(() {
        if (ok) {
          _trustState = TrustState.trusted;
          _recordLog('Trusted auth accepted by host');
        } else if (revoked) {
          _trustState = TrustState.revoked;
          _message = 'This device has been revoked by the host.';
          _recordLog('Trusted auth rejected: revoked');
        } else {
          _message = 'Trusted auth failed: $resultMessage';
          _recordLog('Trusted auth failed: $resultMessage');
        }
      });
      unawaited(_persistClientPrefs());
      if (!ok) {
        _showSnack(revoked ? 'Revoked by host' : 'Trusted auth failed');
      }
      return;
    }

    if (type == 'host_error') {
      final source = message['source'] as String? ?? 'host';
      final error = message['message'] as String? ?? 'Unknown host error';
      setState(() {
        _lastError = '$source: $error';
        _message = 'Host error: $source: $error';
        _recordLog('Host error from $source: $error');
      });
      return;
    }

    if (type == 'file_transfer_progress') {
      final percent = (message['progress_percent'] as num?)?.toDouble();
      final filename = message['filename'] as String? ?? 'file';
      setState(() {
        _transferProgress = percent == null ? null : (percent / 100.0).clamp(0.0, 1.0);
        _transferStatus = percent == null ? 'Receiving $filename...' : 'Receiving $filename (${percent.round()}%)';
      });
      return;
    }

    if (type == 'file_transfer_result') {
      final success = message['success'] == true;
      final filename = message['filename'] as String? ?? 'file';
      final savedPath = message['saved_path'] as String?;
      final error = message['error'] as String?;
      setState(() {
        _transferProgress = success ? 1.0 : null;
        _transferStatus = success
            ? 'Transfer complete: $filename'
            : 'Transfer failed: ${error ?? filename}';
        _message = success
            ? 'Host saved $filename${savedPath == null ? '' : ' -> $savedPath'}'
            : 'Host rejected file: ${error ?? filename}';
        _recordLog(success
            ? 'Host saved $filename${savedPath == null ? '' : ' -> $savedPath'}'
            : 'Host rejected file: ${error ?? filename}');
      });
      return;
    }

    if (type == 'file_transfer_started') {
      final filename = message['filename'] as String? ?? 'file';
      setState(() {
        _transferStatus = 'Host accepted transfer for $filename';
        _recordLog('Host accepted transfer for $filename');
      });
    }
  }

  ConnectionStage _mapErrorToConnectionStage(Object error) {
    final text = error.toString().toLowerCase();
    if (text.contains('handshake') || text.contains('tls')) {
      return ConnectionStage.tlsFailed;
    }
    if (text.contains('auth') || text.contains('token mismatch') || text.contains('not authenticated')) {
      return ConnectionStage.authFailed;
    }
    return ConnectionStage.error;
  }

  String _truncate(String text, int maxChars) {
    if (text.length <= maxChars) {
      return text;
    }
    return '${text.substring(0, maxChars)}...';
  }

  String _trustStateLabel(TrustState state) {
    return switch (state) {
      TrustState.unpaired => 'Unpaired',
      TrustState.pending => 'Pair request pending',
      TrustState.trusted => 'Trusted',
      TrustState.rejected => 'Rejected',
      TrustState.revoked => 'Revoked',
    };
  }

  String _stageLabel(ConnectionStage stage) {
    return switch (stage) {
      ConnectionStage.idle => 'idle',
      ConnectionStage.connecting => 'connecting',
      ConnectionStage.tlsHandshake => 'tls handshake',
      ConnectionStage.authenticating => 'authenticating',
      ConnectionStage.connected => 'connected',
      ConnectionStage.reconnecting => 'reconnecting',
      ConnectionStage.authFailed => 'auth failed',
      ConnectionStage.tlsFailed => 'TLS failed',
      ConnectionStage.disconnected => 'disconnected',
      ConnectionStage.stopped => 'stopped',
      ConnectionStage.error => 'error',
    };
  }

  void _recordLog(String message) {
    final timestamp = DateTime.now().toIso8601String().substring(11, 19);
    final next = <String>[..._clientLogs, '[$timestamp] $message'];
    if (next.length > 80) {
      next.removeRange(0, next.length - 80);
    }
    _clientLogs = next;
  }

  Future<void> _requestResyncWithGuard(String reason) async {
    if (_awaitingResyncKeyframe) {
      _recordLog('Resync suppressed while awaiting keyframe: $reason');
      return;
    }
    _awaitingResyncKeyframe = true;
    _lastResyncRequestAt = DateTime.now();
    _recordLog('Resync requested: $reason');
    await _remoteClient?.requestResync();
  }

  @override
  Widget build(BuildContext context) {
    final media = MediaQuery.of(context);
    final frameHeight = (media.size.height * 0.4).clamp(180.0, 420.0).toDouble();
    final landscapeFullscreen = media.orientation == Orientation.landscape && _hasRenderedFrame;

    if (landscapeFullscreen) {
      return Scaffold(
        backgroundColor: Colors.black,
        floatingActionButton: FloatingActionButton.extended(
          onPressed: _openKeyboardDialog,
          icon: const Icon(Icons.keyboard),
          label: const Text('Keyboard'),
        ),
        body: Stack(
          children: [
            Positioned.fill(
              child: SafeArea(
                child: _buildRemoteFrameViewport(
                  fit: BoxFit.fill,
                  borderRadius: BorderRadius.zero,
                ),
              ),
            ),
            Positioned(
              top: 12,
              left: 12,
              right: 12,
              child: SafeArea(
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    color: Colors.black.withValues(alpha: 0.65),
                    borderRadius: BorderRadius.circular(12),
                  ),
                  child: Padding(
                    padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
                    child: Row(
                      children: [
                        Expanded(
                          child: Text(
                            _useRelayMode
                                ? 'Relay ${_hostController.text.trim()}:${_portController.text.trim()}'
                                : 'Direct ${_hostController.text.trim()}:${_portController.text.trim()}',
                            style: const TextStyle(color: Colors.white, fontWeight: FontWeight.w600),
                            overflow: TextOverflow.ellipsis,
                          ),
                        ),
                        const SizedBox(width: 12),
                        Text(
                          _stageLabel(_connectionStage),
                          style: TextStyle(
                            color: _connected ? Colors.lightGreenAccent : Colors.orangeAccent,
                            fontWeight: FontWeight.bold,
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ],
        ),
      );
    }

    return Scaffold(
      appBar: AppBar(
        title: const Text('AetherLink'),
        actions: [
          IconButton(
            icon: Icon(_localViewOnly ? Icons.visibility_off : Icons.visibility),
            tooltip: _localViewOnly ? 'Local view-only enabled' : 'Local view-only disabled',
            onPressed: () {
              setState(() {
                _localViewOnly = !_localViewOnly;
                _message = _localViewOnly ? 'Local view-only enabled.' : 'Local view-only disabled.';
              });
            },
          ),
        ],
      ),
      floatingActionButton: FloatingActionButton.extended(
        onPressed: _openKeyboardDialog,
        icon: const Icon(Icons.keyboard),
        label: const Text('Keyboard'),
      ),
      body: SafeArea(
        child: ListView(
          padding: EdgeInsets.fromLTRB(16, 16, 16, 120 + media.viewInsets.bottom),
          children: [
            _buildConnectionSummary(),
            const SizedBox(height: 12),
            _buildDiagnosticsPanel(),
            const SizedBox(height: 12),
            _buildSavedHostsPanel(),
            const SizedBox(height: 12),
            _buildTrustPanel(),
            const SizedBox(height: 12),
            TextField(
              controller: _hostController,
              decoration: const InputDecoration(
                labelText: 'Host',
                border: OutlineInputBorder(),
              ),
            ),
            const SizedBox(height: 12),
            TextField(
              controller: _portController,
              keyboardType: TextInputType.number,
              decoration: const InputDecoration(
                labelText: 'Port',
                border: OutlineInputBorder(),
              ),
            ),
            const SizedBox(height: 12),
            TextField(
              controller: _tokenController,
              obscureText: true,
              decoration: const InputDecoration(
                labelText: 'Token',
                border: OutlineInputBorder(),
              ),
            ),
            const SizedBox(height: 12),
            SwitchListTile(
              contentPadding: EdgeInsets.zero,
              title: const Text('Use Relay Mode'),
              value: _useRelayMode,
              onChanged: (value) {
                setState(() {
                  _useRelayMode = value;
                });
              },
            ),
            if (_useRelayMode) ...[
              const SizedBox(height: 8),
              TextField(
                controller: _relayHostIdController,
                decoration: const InputDecoration(
                  labelText: 'Relay Host ID',
                  border: OutlineInputBorder(),
                ),
              ),
              const SizedBox(height: 12),
              TextField(
                controller: _relayTokenController,
                obscureText: true,
                decoration: const InputDecoration(
                  labelText: 'Relay Access Token',
                  border: OutlineInputBorder(),
                ),
              ),
              const SizedBox(height: 12),
            ],
            Row(
              children: [
                Expanded(
                  child: ElevatedButton(
                    onPressed: _connecting ? null : _connectAndStream,
                    child: Text(_connecting ? 'Connecting...' : (_connected ? 'Reconnect' : 'Connect')),
                  ),
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: ElevatedButton(
                    onPressed: _connected ? () => _disconnect(manual: true) : null,
                    child: const Text('Disconnect'),
                  ),
                ),
              ],
            ),
            const SizedBox(height: 8),
            _buildSettingsPanel(),
            const SizedBox(height: 8),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                ElevatedButton(
                  onPressed: _connected ? _sendClipboardToHost : null,
                  child: const Text('Send Clipboard to PC'),
                ),
                ElevatedButton(
                  onPressed: _connected ? _requestClipboardFromHost : null,
                  child: const Text('Pull Clipboard from PC'),
                ),
                ElevatedButton(
                  onPressed: _connected ? _pickAndSendFile : null,
                  child: const Text('Pick File'),
                ),
                ElevatedButton(
                  onPressed: _connected ? () => _sendScroll(120) : null,
                  child: const Text('Scroll Up'),
                ),
                ElevatedButton(
                  onPressed: _connected ? () => _sendScroll(-120) : null,
                  child: const Text('Scroll Down'),
                ),
              ],
            ),
            const SizedBox(height: 8),
            if (_transferStatus != null)
              Card(
                child: Padding(
                  padding: const EdgeInsets.all(12),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(_transferStatus!, style: const TextStyle(fontWeight: FontWeight.bold)),
                      if (_transferProgress != null) ...[
                        const SizedBox(height: 8),
                        LinearProgressIndicator(value: _transferProgress),
                      ],
                      const SizedBox(height: 8),
                      ElevatedButton(
                        onPressed: (_transferProgress != null && _transferProgress! < 1.0 && !_transferCancelled)
                            ? _cancelTransfer
                            : null,
                        child: const Text('Cancel Transfer'),
                      ),
                    ],
                  ),
                ),
              ),
            const SizedBox(height: 12),
            if (_message != null)
              Text(
                _message!,
                style: TextStyle(
                  color: _message!.toLowerCase().contains('failed') || _message!.toLowerCase().contains('error')
                      ? Colors.red
                      : Colors.green.shade700,
                ),
              ),
            if (_hasRenderedFrame)
              Padding(
                padding: const EdgeInsets.only(top: 12),
                child: SizedBox(
                  height: frameHeight,
                  child: _buildRemoteFrameViewport(
                    fit: BoxFit.contain,
                    borderRadius: BorderRadius.circular(8),
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }

  Widget _buildRemoteFrameViewport({
    required BoxFit fit,
    required BorderRadius borderRadius,
  }) {
    return LayoutBuilder(
      builder: (context, imageConstraints) {
        return Listener(
          behavior: HitTestBehavior.opaque,
          onPointerDown: (event) => _handlePointerDown(event.localPosition, imageConstraints),
          onPointerMove: (event) => _handlePointerMove(event.localPosition, imageConstraints),
          onPointerUp: (event) => _handlePointerUp(event.localPosition, imageConstraints),
          child: GestureDetector(
            behavior: HitTestBehavior.opaque,
            onLongPressStart: (details) => _handleLongPressStart(details, imageConstraints),
            child: DecoratedBox(
              decoration: BoxDecoration(
                border: Border.all(color: Colors.black26),
                borderRadius: borderRadius,
              ),
              child: ClipRRect(
                borderRadius: borderRadius,
                child: RepaintBoundary(
                  child: ValueListenableBuilder<ui.Image?>(
                    valueListenable: _framePresenter.imageListenable,
                    builder: (context, image, _) {
                      return ValueListenableBuilder<Size?>(
                        valueListenable: _framePresenter.frameSizeListenable,
                        builder: (context, frameSize, _) {
                          if (image == null || frameSize == null || frameSize.width <= 0 || frameSize.height <= 0) {
                            return const SizedBox.expand();
                          }
                          final viewportSignature =
                              '${frameSize.width.toInt()}x${frameSize.height.toInt()}|${imageConstraints.maxWidth.toStringAsFixed(0)}x${imageConstraints.maxHeight.toStringAsFixed(0)}|${MediaQuery.of(context).orientation.name}';
                          if (_lastViewportSignature != viewportSignature) {
                            _lastViewportSignature = viewportSignature;
                            _recordLog(
                              'Render viewport: frame=${frameSize.width.toInt()}x${frameSize.height.toInt()} constraints=${imageConstraints.maxWidth.toStringAsFixed(0)}x${imageConstraints.maxHeight.toStringAsFixed(0)} fit=contain orientation=${MediaQuery.of(context).orientation.name}',
                            );
                          }
                          return Center(
                            child: FittedBox(
                              fit: BoxFit.contain,
                              child: SizedBox(
                                width: frameSize.width,
                                height: frameSize.height,
                                child: RawImage(
                                  image: image,
                                  fit: BoxFit.contain,
                                  filterQuality: FilterQuality.none,
                                ),
                              ),
                            ),
                          );
                        },
                      );
                    },
                  ),
                ),
              ),
            ),
          ),
        );
      },
    );
  }

  Widget _buildConnectionSummary() {
    final color = switch (_connectionStage) {
      ConnectionStage.connected => Colors.green.shade700,
      ConnectionStage.connecting ||
      ConnectionStage.tlsHandshake ||
      ConnectionStage.authenticating ||
      ConnectionStage.reconnecting => Colors.orange.shade700,
      ConnectionStage.authFailed || ConnectionStage.tlsFailed || ConnectionStage.error => Colors.red.shade700,
      ConnectionStage.disconnected || ConnectionStage.idle || ConnectionStage.stopped => Colors.grey.shade700,
    };
    final text = _stageLabel(_connectionStage);
    return Row(
      children: [
        Icon(Icons.lan, color: color),
        const SizedBox(width: 8),
        Expanded(
          child: Text(
            _hasRenderedFrame ? '$text, frame age ${_lastRenderedFrameAgeMs}ms' : text,
            style: TextStyle(fontWeight: FontWeight.bold, color: color),
          ),
        ),
      ],
    );
  }

  Widget _buildDiagnosticsPanel() {
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text('Diagnostics', style: TextStyle(fontWeight: FontWeight.bold)),
            const SizedBox(height: 8),
            Text('Host: ${_hostController.text.trim().isEmpty ? '-' : _hostController.text.trim()}'),
            Text('Port: ${_portController.text.trim().isEmpty ? '-' : _portController.text.trim()}'),
            Text('Transport: ${_useRelayMode ? 'relay' : 'direct'}'),
            if (_useRelayMode) Text('Relay host ID: ${_relayHostIdController.text.trim().isEmpty ? '-' : _relayHostIdController.text.trim()}'),
            Text('State: ${_stageLabel(_connectionStage)}'),
            Text('Monitor: ${_settings.monitorIndex}'),
            if (_monitorLabels.isNotEmpty)
              Text('Host monitors: ${_monitorLabels.values.join(' | ')}'),
            Text('Settings: ${_settings.targetWidth}px, ${_settings.fps} FPS, JPEG ${_settings.jpegQuality.round()}, view-only=${_settings.viewOnly}, clipboard=${_clipboardModeToWire(_settings.clipboardMode)}, delta=${_settings.deltaStreamEnabled}, audio=${_settings.audioEnabled}'),
            Text('Trust: ${_trustStateLabel(_trustState)}${_trustedDeviceId == null ? '' : ' (${_truncate(_trustedDeviceId!, 18)})'}'),
            Text('Auto reconnect: ${_autoReconnectEnabled ? 'on' : 'off'}'),
            Text('Reconnect attempts: $_reconnectAttempt/${_reconnectBackoffSeconds.length}'),
            if (_hostClipboardText != null) Text('Host clipboard: ${_truncate(_hostClipboardText!, 60)}'),
            if (_lastError != null) Text('Last error: $_lastError'),
            const SizedBox(height: 8),
            SwitchListTile(
              contentPadding: EdgeInsets.zero,
              title: const Text('Auto reconnect'),
              value: _autoReconnectEnabled,
              onChanged: (value) {
                setState(() {
                  _autoReconnectEnabled = value;
                });
                unawaited(_persistClientPrefs());
              },
            ),
            Row(
              children: [
                ElevatedButton(
                  onPressed: () async {
                    await Clipboard.setData(ClipboardData(text: _clientLogs.join('\n')));
                    if (!mounted) {
                      return;
                    }
                    setState(() {
                      _message = 'Client logs copied to clipboard.';
                    });
                  },
                  child: const Text('Copy Logs'),
                ),
              ],
            ),
            const SizedBox(height: 8),
            SizedBox(
              height: 140,
              child: ListView.builder(
                itemCount: _clientLogs.length,
                itemBuilder: (context, index) => Text(
                  _clientLogs[index],
                  style: Theme.of(context).textTheme.bodySmall,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildSavedHostsPanel() {
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text('Saved Hosts', style: TextStyle(fontWeight: FontWeight.bold)),
            const SizedBox(height: 8),
            if (_loadingSavedHosts)
              const Text('Loading saved hosts...')
            else if (_savedHosts.isEmpty)
              const Text('No saved connections yet.')
            else
              DecoratedBox(
                decoration: BoxDecoration(
                  border: Border.all(color: Colors.black26),
                  borderRadius: BorderRadius.circular(8),
                ),
                child: Column(
                  children: _savedHosts.map((entry) {
                    final selected = entry.label == _selectedSavedHostLabel;
                    return ListTile(
                      dense: true,
                      selected: selected,
                      leading: Icon(selected ? Icons.radio_button_checked : Icons.radio_button_off),
                      title: Text(entry.label),
                      subtitle: Text(
                        '${entry.host}:${entry.port}${entry.useRelayMode ? '  relay:${entry.relayHostId}' : ''}',
                      ),
                      onTap: () => _applySavedHost(entry),
                    );
                  }).toList(),
                ),
              ),
            const SizedBox(height: 8),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                ElevatedButton(
                  onPressed: () => _saveCurrentHost(),
                  child: const Text('Save Current'),
                ),
                ElevatedButton(
                  onPressed: _selectedSavedHostLabel == null ? null : () => _saveCurrentHost(updateExisting: true),
                  child: const Text('Update'),
                ),
                OutlinedButton(
                  onPressed: _selectedSavedHostLabel == null ? null : _deleteSelectedHost,
                  child: const Text('Delete'),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildTrustPanel() {
    final trustColor = switch (_trustState) {
      TrustState.trusted => Colors.green.shade700,
      TrustState.pending => Colors.orange.shade700,
      TrustState.revoked || TrustState.rejected => Colors.red.shade700,
      TrustState.unpaired => Colors.grey.shade700,
    };
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text('Trust / Pairing', style: TextStyle(fontWeight: FontWeight.bold)),
            const SizedBox(height: 8),
            Text('Device: $_trustedDeviceName'),
            if (_trustedDeviceId != null) Text('Device ID: ${_truncate(_trustedDeviceId!, 32)}'),
            Text('State: ${_trustStateLabel(_trustState)}', style: TextStyle(color: trustColor, fontWeight: FontWeight.bold)),
            const SizedBox(height: 8),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                ElevatedButton(
                  onPressed: _connected ? () => _pairThisDevice() : null,
                  child: const Text('Pair this device'),
                ),
                ElevatedButton(
                  onPressed: _connected ? () => _pairThisDevice(forceRePair: true) : null,
                  child: const Text('Re-pair'),
                ),
                OutlinedButton(
                  onPressed: _forgetLocalTrustMetadata,
                  child: const Text('Forget local trust metadata'),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildSettingsPanel() {
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text('Session Settings', style: TextStyle(fontWeight: FontWeight.bold)),
            const SizedBox(height: 8),
            DropdownButtonFormField<ResolutionPreset>(
              initialValue: _settings.resolution,
              decoration: const InputDecoration(labelText: 'Resolution Preset'),
              items: const [
                DropdownMenuItem(value: ResolutionPreset.low, child: Text('Responsive (854)')),
                DropdownMenuItem(value: ResolutionPreset.medium, child: Text('Balanced (960)')),
                DropdownMenuItem(value: ResolutionPreset.high, child: Text('Quality (1280)')),
              ],
              onChanged: (value) {
                if (value != null) {
                  setState(() => _settings.resolution = value);
                  unawaited(_persistClientPrefs());
                }
              },
            ),
            const SizedBox(height: 8),
            DropdownButtonFormField<int>(
              initialValue: _settings.fps,
              decoration: const InputDecoration(labelText: 'FPS'),
              items: const [10, 15, 20, 30]
                  .map((v) => DropdownMenuItem<int>(value: v, child: Text('$v')))
                  .toList(),
              onChanged: (value) {
                if (value != null) {
                  setState(() => _settings.fps = value);
                  unawaited(_persistClientPrefs());
                }
              },
            ),
            const SizedBox(height: 8),
            DropdownButtonFormField<int>(
              initialValue: _settings.monitorIndex,
              decoration: const InputDecoration(labelText: 'Monitor Index'),
              items: _availableMonitorIndexes
                  .map((v) => DropdownMenuItem<int>(
                        value: v,
                        child: Text(_monitorLabels[v] ?? 'Monitor $v'),
                      ))
                  .toList(),
              onChanged: (value) {
                if (value != null) {
                  setState(() => _settings.monitorIndex = value);
                  unawaited(_persistClientPrefs());
                }
              },
            ),
            const SizedBox(height: 8),
            Text('JPEG Quality: ${_settings.jpegQuality.round()}'),
            Slider(
              value: _settings.jpegQuality,
              min: 40,
              max: 90,
              divisions: 50,
              label: _settings.jpegQuality.round().toString(),
              onChanged: (value) {
                setState(() => _settings.jpegQuality = value);
                unawaited(_persistClientPrefs());
              },
            ),
            SwitchListTile(
              title: const Text('View-only (remote session)'),
              value: _settings.viewOnly,
              onChanged: (value) {
                setState(() => _settings.viewOnly = value);
                unawaited(_persistClientPrefs());
              },
            ),
            SwitchListTile(
              title: const Text('Delta Stream Updates'),
              value: _settings.deltaStreamEnabled,
              onChanged: (value) {
                setState(() => _settings.deltaStreamEnabled = value);
                unawaited(_persistClientPrefs());
              },
            ),
            DropdownButtonFormField<ClipboardSyncMode>(
              initialValue: _settings.clipboardMode,
              decoration: const InputDecoration(labelText: 'Clipboard Sync Mode'),
              items: const [
                DropdownMenuItem(value: ClipboardSyncMode.manual, child: Text('Manual')),
                DropdownMenuItem(value: ClipboardSyncMode.hostToClient, child: Text('Auto: Host to Client')),
              ],
              onChanged: (value) {
                if (value != null) {
                  setState(() => _settings.clipboardMode = value);
                  unawaited(_persistClientPrefs());
                }
              },
            ),
            SwitchListTile(
              title: const Text('Audio Streaming (host advertises only)'),
              value: _settings.audioEnabled,
              onChanged: (value) {
                setState(() => _settings.audioEnabled = value);
                unawaited(_persistClientPrefs());
              },
            ),
            const SizedBox(height: 8),
            ElevatedButton(
              onPressed: _connected ? _applySettings : null,
              child: const Text('Apply Settings'),
            ),
          ],
        ),
      ),
    );
  }
}

class _KeyboardInputPage extends StatefulWidget {
  const _KeyboardInputPage();

  @override
  State<_KeyboardInputPage> createState() => _KeyboardInputPageState();
}

class _KeyboardInputPageState extends State<_KeyboardInputPage> {
  final TextEditingController _controller = TextEditingController();

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Send Keyboard Input')),
      body: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            TextField(
              controller: _controller,
              autofocus: true,
              decoration: const InputDecoration(
                hintText: 'Type text to send',
                border: OutlineInputBorder(),
              ),
              onSubmitted: (value) {
                Navigator.of(context).pop('submit:$value');
              },
            ),
            const SizedBox(height: 12),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                ElevatedButton(
                  onPressed: () => Navigator.of(context).pop('send:${_controller.text}'),
                  child: const Text('Send'),
                ),
                ElevatedButton(
                  onPressed: () => Navigator.of(context).pop('enter'),
                  child: const Text('Enter'),
                ),
                ElevatedButton(
                  onPressed: () => Navigator.of(context).pop('backspace'),
                  child: const Text('Backspace'),
                ),
                ElevatedButton(
                  onPressed: () => Navigator.of(context).pop('tab'),
                  child: const Text('Tab'),
                ),
                ElevatedButton(
                  onPressed: () => Navigator.of(context).pop('esc'),
                  child: const Text('Esc'),
                ),
                ElevatedButton(
                  onPressed: () => Navigator.of(context).pop('left'),
                  child: const Text('Left'),
                ),
                ElevatedButton(
                  onPressed: () => Navigator.of(context).pop('up'),
                  child: const Text('Up'),
                ),
                ElevatedButton(
                  onPressed: () => Navigator.of(context).pop('right'),
                  child: const Text('Right'),
                ),
                ElevatedButton(
                  onPressed: () => Navigator.of(context).pop('down'),
                  child: const Text('Down'),
                ),
                ElevatedButton(
                  onPressed: () => Navigator.of(context).pop('ctrl+c'),
                  child: const Text('Ctrl+C'),
                ),
                ElevatedButton(
                  onPressed: () => Navigator.of(context).pop('ctrl+v'),
                  child: const Text('Ctrl+V'),
                ),
                ElevatedButton(
                  onPressed: () => Navigator.of(context).pop('ctrl+a'),
                  child: const Text('Ctrl+A'),
                ),
                ElevatedButton(
                  onPressed: () => Navigator.of(context).pop('alt+tab'),
                  child: const Text('Alt+Tab'),
                ),
                OutlinedButton(
                  onPressed: () => Navigator.of(context).pop('cancel'),
                  child: const Text('Cancel'),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}




