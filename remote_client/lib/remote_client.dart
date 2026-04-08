import 'dart:async';
import 'dart:convert';
import 'dart:isolate';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';
import 'dart:collection';

import 'package:crypto/crypto.dart';
import 'package:flutter/services.dart';
import 'package:image/image.dart' as img;

const String kPinnedServerSha256Fingerprint =
    '9F:E9:2B:BC:11:F7:B8:1F:8E:0B:EE:EE:39:88:C9:7E:5F:B3:CC:CB:FD:A2:BB:3B:3A:D1:B4:1B:90:EE:46:59';
const bool kAllowInsecureLocalTlsForTesting = true;
const int _msgVideoFrame = 0x01;
const int _msgControlInput = 0x02;
const int _msgVideoKeyframe = 0x03;
const int _msgVideoDelta = 0x04;
const int _msgAudioPacket = 0x05;
const int _codecJpeg = 0x01;
const int _audioCodecPcmS16Le = 0x00;
const int _frameLogSampleEvery = 30;
const int _maxPendingAudioPackets = 6;
const int _audioMaxQueueAgeMs = 100;
const int _latencySummaryIntervalMs = 5000;
const bool kDebugAudioOnlyMode = bool.fromEnvironment(
  'AETHERLINK_AUDIO_ONLY_MODE',
  defaultValue: false,
);
const bool kDebugVideoDisabled = bool.fromEnvironment(
  'AETHERLINK_VIDEO_DISABLED',
  defaultValue: false,
);
const bool kDebugAudioDisabled = bool.fromEnvironment(
  'AETHERLINK_AUDIO_DISABLED',
  defaultValue: false,
);
const bool kDebugVideoDecodeBypass = bool.fromEnvironment(
  'AETHERLINK_VIDEO_DECODE_BYPASS',
  defaultValue: false,
);
const bool kDisableStartupStaleDrop = bool.fromEnvironment(
  'AETHERLINK_DISABLE_STARTUP_STALE_DROP',
  defaultValue: false,
);

class RemoteVideoFrame {
  RemoteVideoFrame({
    required this.frameId,
    required this.width,
    required this.height,
    required this.rgbaBytes,
    required this.captureTimestampMs,
    required this.receivedTimestampMs,
    required this.messageType,
    required this.assemblyCompletedTimestampMs,
    required this.decodeStartTimestampMs,
    required this.decodeEndTimestampMs,
    this.isPlaceholder = false,
  });

  final int frameId;
  final int width;
  final int height;
  final Uint8List rgbaBytes;
  final int captureTimestampMs;
  final int receivedTimestampMs;
  final int messageType;
  final int assemblyCompletedTimestampMs;
  final int decodeStartTimestampMs;
  final int decodeEndTimestampMs;
  final bool isPlaceholder;

  int get queueAgeMs =>
      DateTime.now().millisecondsSinceEpoch - receivedTimestampMs;
  int get hostClockAgeMs =>
      DateTime.now().millisecondsSinceEpoch - captureTimestampMs;
  int get receiveToDecodeStartMs =>
      decodeStartTimestampMs - receivedTimestampMs;
  int get decodeMs => decodeEndTimestampMs - decodeStartTimestampMs;

  bool get isKeyframe => messageType == _msgVideoKeyframe;
}

class _PendingCompressedFrame {
  const _PendingCompressedFrame({
    required this.messageType,
    required this.payload,
    required this.frameId,
    required this.receivedTimestampMs,
    required this.assemblyCompletedTimestampMs,
  });

  final int messageType;
  final Uint8List payload;
  final int frameId;
  final int receivedTimestampMs;
  final int assemblyCompletedTimestampMs;
}

class _PendingAudioPacket {
  const _PendingAudioPacket({
    required this.payload,
    required this.receivedTimestampMs,
    required this.enqueueTimestampMs,
    required this.ptsMs,
    required this.sequence,
    required this.queueDepthAtEnqueue,
  });

  final Uint8List payload;
  final int receivedTimestampMs;
  final int enqueueTimestampMs;
  final int ptsMs;
  final int sequence;
  final int queueDepthAtEnqueue;

  int get queueAgeMs =>
      DateTime.now().millisecondsSinceEpoch - receivedTimestampMs;
}

class _SlidingIntStats {
  _SlidingIntStats();

  static const int _maxSamples = 240;
  final ListQueue<int> _samples = ListQueue<int>();

  void add(int value) {
    _samples.addLast(value);
    while (_samples.length > _maxSamples) {
      _samples.removeFirst();
    }
  }

  bool get isEmpty => _samples.isEmpty;

  double get average {
    if (_samples.isEmpty) {
      return 0;
    }
    final total = _samples.fold<int>(0, (sum, value) => sum + value);
    return total / _samples.length;
  }

  String get averageLabel {
    if (_samples.isEmpty) {
      return '-';
    }
    return average.toStringAsFixed(1);
  }

  int get p95 {
    if (_samples.isEmpty) {
      return 0;
    }
    final sorted = _samples.toList()..sort();
    final index = ((sorted.length - 1) * 0.95).floor();
    return sorted[index];
  }
}

class _InputLatencyProbe {
  const _InputLatencyProbe({
    required this.kind,
    required this.sentAt,
    required this.frameIdBeforeSend,
  });

  final String kind;
  final DateTime sentAt;
  final int frameIdBeforeSend;
}

class TrustedDeviceIdentity {
  const TrustedDeviceIdentity({
    required this.deviceId,
    required this.deviceName,
    required this.keystoreAlias,
    required this.publicKeyPem,
  });

  final String deviceId;
  final String deviceName;
  final String keystoreAlias;
  final String publicKeyPem;
}

class AndroidTrustIdentityService {
  AndroidTrustIdentityService._();

  static final AndroidTrustIdentityService instance =
      AndroidTrustIdentityService._();
  static const MethodChannel _channel = MethodChannel('aetherlink/trust');

  Future<TrustedDeviceIdentity> getOrCreateDeviceIdentity() async {
    final raw = await _channel.invokeMapMethod<String, dynamic>(
      'getOrCreateDeviceIdentity',
    );
    if (raw == null) {
      throw PlatformException(
        code: 'trust_identity_failed',
        message: 'Missing identity payload',
      );
    }
    return TrustedDeviceIdentity(
      deviceId: raw['deviceId'] as String? ?? '',
      deviceName: raw['deviceName'] as String? ?? 'Android Device',
      keystoreAlias: raw['keystoreAlias'] as String? ?? '',
      publicKeyPem: raw['publicKeyPem'] as String? ?? '',
    );
  }

  Future<Uint8List> signChallenge(Uint8List payload) async {
    final signature = await _channel.invokeMethod<Uint8List>(
      'signChallenge',
      <String, Object>{'payload': payload},
    );
    if (signature == null || signature.isEmpty) {
      throw PlatformException(
        code: 'trust_sign_failed',
        message: 'Empty signature',
      );
    }
    return signature;
  }

  Future<void> forgetLocalIdentity() async {
    await _channel.invokeMethod<void>('forgetLocalIdentity');
  }
}

class _AndroidAudioSink {
  static const MethodChannel _channel = MethodChannel('aetherlink/audio');
  bool _started = false;
  int? _sampleRate;
  int? _channels;

  Future<void> playPcm16({
    required int sampleRate,
    required int channels,
    required Uint8List data,
  }) async {
    if (!_started || _sampleRate != sampleRate || _channels != channels) {
      await _channel.invokeMethod<void>('startPcm16', <String, Object>{
        'sampleRate': sampleRate,
        'channels': channels,
      });
      print(
        '[audio] playback started sampleRate=$sampleRate channels=$channels',
      );
      _started = true;
      _sampleRate = sampleRate;
      _channels = channels;
    }
    await _channel.invokeMethod<void>('writePcm16', <String, Object>{
      'data': data,
    });
  }

  Future<void> stop() async {
    if (!_started) {
      return;
    }
    await _channel.invokeMethod<void>('stopAudio');
    print('[audio] playback stopped');
    _started = false;
    _sampleRate = null;
    _channels = null;
  }

  Future<Map<String, dynamic>> getPlaybackStats() async {
    final raw = await _channel.invokeMapMethod<String, dynamic>(
      'getPlaybackStats',
    );
    return raw ?? const <String, dynamic>{};
  }
}

class RemoteClient {
  final String host;
  final int port;
  final String authToken;
  final String? relayHostId;
  final String? relayToken;
  SecureSocket? _socket;
  StreamIterator<Uint8List>? _iterator;
  final StreamController<Map<String, dynamic>> _controlMessages =
      StreamController<Map<String, dynamic>>.broadcast();
  final StreamController<RemoteVideoFrame> _videoFrames =
      StreamController<RemoteVideoFrame>.broadcast();
  final ListQueue<String> _recentClipboardSyncIds = ListQueue<String>();
  String _clipboardMode = 'manual';
  String? _lastAppliedClipboardHash;
  DateTime? _lastAppliedClipboardAt;
  Uint8List _pending = Uint8List(0);
  int _pendingOffset = 0;
  bool _authenticated = false;
  final _AndroidAudioSink _audioSink = _AndroidAudioSink();
  final AndroidTrustIdentityService _trustIdentityService =
      AndroidTrustIdentityService.instance;
  TrustedDeviceIdentity? _trustedIdentity;
  Uint8List? _pendingPairClientNonce;
  bool _trustedAuthEnabled = false;
  String? _activeTransferId;
  final Map<String, Completer<void>> _pendingTransferStarts =
      <String, Completer<void>>{};
  final Map<String, Map<int, Completer<void>>> _pendingTransferChunkAcks =
      <String, Map<int, Completer<void>>>{};
  Isolate? _frameDecodeIsolate;
  ReceivePort? _frameDecodeReceivePort;
  SendPort? _frameDecodeSendPort;
  final Map<int, Completer<Map<String, Object?>>> _pendingDecodeResponses =
      <int, Completer<Map<String, Object?>>>{};
  int _nextDecodeRequestId = 1;
  bool _readLoopRunning = false;
  bool _decodeLoopRunning = false;
  bool _controlDispatchRunning = false;
  _PendingCompressedFrame? _pendingCompressedFrame;
  int _replacedCompressedFrameCount = 0;
  int _staleDropsAfterDecode = 0;
  int _videoFramesDecodeBypassed = 0;
  final Queue<Map<String, dynamic>> _pendingControlMessages =
      Queue<Map<String, dynamic>>();
  final Queue<_PendingAudioPacket> _pendingAudioPackets =
      Queue<_PendingAudioPacket>();
  bool _audioDispatchRunning = false;
  int _audioPacketsReceived = 0;
  int _audioPacketsSubmitted = 0;
  int _audioPacketsDropped = 0;
  int _audioLateDrops = 0;
  int _decodePressureDrops = 0;
  int _targetFrameBudgetMs = 100;
  int _lastDecodedFrameId = 0;
  _InputLatencyProbe? _pendingInputLatencyProbe;
  final _SlidingIntStats _inputToVisibleMs = _SlidingIntStats();
  int _lastVideoSummaryAtMs = 0;
  int _lastAudioSummaryAtMs = 0;
  int _lastTransportSummaryAtMs = 0;
  final _SlidingIntStats _transportDispatchMs = _SlidingIntStats();
  final _SlidingIntStats _videoReceiveToDecodeStartMs = _SlidingIntStats();
  final _SlidingIntStats _videoDecodeMs = _SlidingIntStats();
  final _SlidingIntStats _videoReceiveToEmitMs = _SlidingIntStats();
  final _SlidingIntStats _audioReceiveToSubmitMs = _SlidingIntStats();
  final _SlidingIntStats _audioQueueDepthStats = _SlidingIntStats();
  final _SlidingIntStats _audioBufferOccupancyMs = _SlidingIntStats();

  RemoteClient({
    required this.host,
    required this.port,
    this.authToken = '',
    this.relayHostId,
    this.relayToken,
  });

  Future<void> connect() async {
    if (_socket != null) {
      return;
    }

    final socket = await _connectSocket();
    socket.setOption(SocketOption.tcpNoDelay, true);

    final peerCert = socket.peerCertificate;
    if (relayHostId == null &&
        !kAllowInsecureLocalTlsForTesting &&
        (peerCert == null || !_isPinnedCertificate(peerCert))) {
      await socket.close();
      throw const SocketException('TLS certificate pinning failed');
    }

    _socket = socket;
    _iterator = StreamIterator<Uint8List>(_socket!);

    if (relayHostId != null) {
      await _negotiateRelaySession();
    }

    await _sendControlJson(<String, Object?>{
      'type': 'auth',
      'token': authToken.trim(),
    });
    _authenticated = true;
  }

  Future<SecureSocket> _connectSocket() {
    return SecureSocket.connect(
      host,
      port,
      timeout: const Duration(seconds: 10),
      onBadCertificate: (cert) =>
          kAllowInsecureLocalTlsForTesting || _isPinnedCertificate(cert),
    );
  }

  Future<void> _negotiateRelaySession() async {
    await _sendControlJson(<String, Object?>{
      'type': 'relay_connect_client',
      'host_id': relayHostId!,
      'token': relayToken?.trim(),
      'device_id': _trustedIdentity?.deviceId,
    }, allowUnauthenticated: true);

    while (true) {
      final message = await _readMessage();
      if (message == null) {
        throw const SocketException('Relay closed before session was ready');
      }
      if (message.$1 != _msgControlInput) {
        continue;
      }

      final decoded =
          jsonDecode(utf8.decode(message.$2)) as Map<String, dynamic>;
      final type = decoded['type'] as String?;
      if (type == 'relay_session_ready') {
        _controlMessages.add(decoded);
        return;
      }
      if (type == 'relay_error') {
        throw SocketException(
          decoded['message'] as String? ?? 'Relay connection failed',
        );
      }
      _controlMessages.add(decoded);
    }
  }

  Future<TrustedDeviceIdentity> getOrCreateDeviceIdentity() async {
    _trustedIdentity ??= await _trustIdentityService
        .getOrCreateDeviceIdentity();
    return _trustedIdentity!;
  }

  Future<void> forgetLocalIdentity() async {
    await _trustIdentityService.forgetLocalIdentity();
    _trustedIdentity = null;
    _pendingPairClientNonce = null;
    _trustedAuthEnabled = false;
  }

  void setTrustedAuthEnabled(bool enabled) {
    _trustedAuthEnabled = enabled;
  }

  Future<void> requestPairing({String? deviceName}) async {
    final identity = await getOrCreateDeviceIdentity();
    final clientNonce = _randomBytes(32);
    _pendingPairClientNonce = clientNonce;
    await _sendControlJson(<String, Object?>{
      'type': 'pair_request',
      'device_id': identity.deviceId,
      'device_name': (deviceName?.trim().isNotEmpty ?? false)
          ? deviceName!.trim()
          : identity.deviceName,
      'public_key_pem': identity.publicKeyPem,
      'client_nonce_b64': base64Encode(clientNonce),
    });
  }

  Future<void> close() async {
    for (final transferId in <String>{
      ..._pendingTransferStarts.keys,
      ..._pendingTransferChunkAcks.keys,
    }) {
      _failPendingTransfer(
        transferId,
        const SocketException('connection closed'),
      );
    }
    await _iterator?.cancel();
    await _socket?.close();
    _iterator = null;
    _socket = null;
    _pending = Uint8List(0);
    _pendingOffset = 0;
    _authenticated = false;
    _activeTransferId = null;
    _recentClipboardSyncIds.clear();
    _clipboardMode = 'manual';
    _lastAppliedClipboardHash = null;
    _lastAppliedClipboardAt = null;
    _pendingCompressedFrame = null;
    _pendingAudioPackets.clear();
    _audioDispatchRunning = false;
    _readLoopRunning = false;
    _decodeLoopRunning = false;
    _controlDispatchRunning = false;
    _pendingControlMessages.clear();
    for (final pending in _pendingDecodeResponses.values) {
      if (!pending.isCompleted) {
        pending.completeError(const SocketException('frame decoder closed'));
      }
    }
    _pendingDecodeResponses.clear();
    _frameDecodeReceivePort?.close();
    _frameDecodeReceivePort = null;
    _frameDecodeSendPort = null;
    _frameDecodeIsolate?.kill(priority: Isolate.immediate);
    _frameDecodeIsolate = null;
    await _audioSink.stop();
  }

  Stream<Map<String, dynamic>> get controlMessages => _controlMessages.stream;

  Future<RemoteVideoFrame?> fetchSingleFrame() async {
    await connect();
    return streamFrames().first;
  }

  Stream<RemoteVideoFrame> streamFrames() {
    unawaited(_startReadLoop());
    return _videoFrames.stream;
  }

  Future<void> _startReadLoop() async {
    if (_readLoopRunning) {
      return;
    }
    await connect();
    await _ensureFrameDecodeIsolate();
    _readLoopRunning = true;
    unawaited(() async {
      try {
        while (true) {
          final message = await _readMessage();
          if (message == null) {
            break;
          }
          final dispatchStartMs = DateTime.now().millisecondsSinceEpoch;
          if (message.$1 == _msgControlInput) {
            try {
              final decoded =
                  jsonDecode(utf8.decode(message.$2)) as Map<String, dynamic>;
              _enqueueControlMessage(decoded);
            } catch (_) {
              // Ignore malformed control messages from the host.
            }
            _recordTransportDispatch(dispatchStartMs);
            continue;
          }
          if (message.$1 == _msgAudioPacket) {
            _controlMessages.add(<String, dynamic>{
              'type': 'client_audio_packet',
            });
            _handleAudioPacket(message.$2);
            _recordTransportDispatch(dispatchStartMs);
            continue;
          }
          if (message.$1 == _msgVideoFrame ||
              message.$1 == _msgVideoKeyframe ||
              message.$1 == _msgVideoDelta) {
            _controlMessages.add(<String, dynamic>{
              'type': 'client_video_packet',
              'message_type': message.$1,
              'payload_len': message.$2.length,
            });
            _enqueueCompressedFrame(message.$1, message.$2);
            _recordTransportDispatch(dispatchStartMs);
          }
        }
      } catch (err, stackTrace) {
        if (!_videoFrames.isClosed) {
          _videoFrames.addError(err, stackTrace);
        }
      } finally {
        _readLoopRunning = false;
      }
    }());
  }

  void _enqueueCompressedFrame(int messageType, Uint8List payload) {
    if (kDebugAudioOnlyMode || kDebugVideoDisabled) {
      return;
    }
    final frameId = _peekFrameId(messageType, payload);
    final receivedTimestampMs = DateTime.now().millisecondsSinceEpoch;
    final existing = _pendingCompressedFrame;
    if (existing != null) {
      final existingIsKeyframe = existing.messageType == _msgVideoKeyframe;
      final incomingIsKeyframe = messageType == _msgVideoKeyframe;

      // Never let a pending keyframe baseline get displaced by a newer delta.
      if (existingIsKeyframe && !incomingIsKeyframe) {
        if (frameId % _frameLogSampleEvery == 0) {
          print(
            'video frame dropped before decode old_frame=${existing.frameId} kept_reason=pending_keyframe new_frame=$frameId dropped_reason=awaiting_keyframe_decode',
          );
        }
        return;
      }

      _replacedCompressedFrameCount += 1;
      if (frameId % _frameLogSampleEvery == 0) {
        print(
          'video frame replaced before decode old_frame=${existing.frameId} new_frame=$frameId dropped_reason=decode_backlog total_replaced=$_replacedCompressedFrameCount',
        );
      }
    }
    _pendingCompressedFrame = _PendingCompressedFrame(
      messageType: messageType,
      payload: payload,
      frameId: frameId,
      receivedTimestampMs: receivedTimestampMs,
      assemblyCompletedTimestampMs: receivedTimestampMs,
    );
    if (!_decodeLoopRunning) {
      unawaited(_runDecodeLoop());
    }
  }

  Future<void> _runDecodeLoop() async {
    if (_decodeLoopRunning) {
      return;
    }
    _decodeLoopRunning = true;
    try {
      while (true) {
        final pendingFrame = _pendingCompressedFrame;
        if (pendingFrame == null) {
          break;
        }
        _pendingCompressedFrame = null;
        final decodeStartTimestampMs = DateTime.now().millisecondsSinceEpoch;
        _videoReceiveToDecodeStartMs.add(
          decodeStartTimestampMs - pendingFrame.receivedTimestampMs,
        );
        final decoded = kDebugVideoDecodeBypass
            ? _buildDecodeBypassFrame(pendingFrame)
            : await _decodeFrameOnWorker(pendingFrame);
        final decodeEndTimestampMs = DateTime.now().millisecondsSinceEpoch;
        _videoDecodeMs.add(decodeEndTimestampMs - decodeStartTimestampMs);
        if (decoded['request_resync'] == true) {
          print(
            'video frame dropped frame_id=${pendingFrame.frameId} dropped_reason=${decoded['drop_reason'] ?? 'resync_required'}',
          );
          _controlMessages.add(<String, dynamic>{
            'type': 'client_resync_needed',
            'reason': decoded['drop_reason'] ?? 'resync_required',
            'frame_id': pendingFrame.frameId,
          });
          continue;
        }
        final rgbaData = decoded['rgba'] as TransferableTypedData?;
        if (rgbaData == null) {
          continue;
        }
        final frame = RemoteVideoFrame(
          frameId: decoded['frame_id'] as int,
          width: decoded['width'] as int,
          height: decoded['height'] as int,
          rgbaBytes: rgbaData.materialize().asUint8List(),
          captureTimestampMs: decoded['capture_timestamp_ms'] as int,
          receivedTimestampMs: decoded['received_timestamp_ms'] as int,
          messageType: pendingFrame.messageType,
          assemblyCompletedTimestampMs:
              pendingFrame.assemblyCompletedTimestampMs,
          decodeStartTimestampMs: decodeStartTimestampMs,
          decodeEndTimestampMs: decodeEndTimestampMs,
          isPlaceholder: decoded['is_placeholder'] == true,
        );
        _lastDecodedFrameId = frame.frameId;
        final expectedBytes = frame.width * frame.height * 4;
        if (frame.width <= 0 ||
            frame.height <= 0 ||
            frame.rgbaBytes.length != expectedBytes) {
          print(
            '[video] invalid rgba frame_id=${frame.frameId} type=${pendingFrame.messageType} width=${frame.width} height=${frame.height} rgba=${frame.rgbaBytes.length} expected=$expectedBytes dropped_reason=dimension_mismatch',
          );
          unawaited(requestResync());
          continue;
        }
        final receiveToEmitMs =
            DateTime.now().millisecondsSinceEpoch - frame.receivedTimestampMs;
        _videoReceiveToEmitMs.add(receiveToEmitMs);
        if (_shouldDropDeltaUnderDecodePressure(frame.messageType)) {
          _decodePressureDrops += 1;
          if (frame.frameId % _frameLogSampleEvery == 0) {
            print(
              '[video] decode-pressure drop frame_id=${frame.frameId} decode_avg=${_videoDecodeMs.averageLabel}ms decode_p95=${_videoDecodeMs.p95}ms budget_ms=$_targetFrameBudgetMs drops=$_decodePressureDrops',
            );
          }
          unawaited(requestResync());
          _maybeEmitVideoSummary();
          continue;
        }
        if (_pendingCompressedFrame != null &&
            receiveToEmitMs > max(_targetFrameBudgetMs, 80) &&
            !frame.isKeyframe) {
          _staleDropsAfterDecode += 1;
          if (frame.frameId % _frameLogSampleEvery == 0) {
            print(
              '[video] stale drop after decode frame_id=${frame.frameId} receive_to_emit_ms=$receiveToEmitMs pending_frame=${_pendingCompressedFrame!.frameId} stale_drops=$_staleDropsAfterDecode',
            );
          }
          _maybeEmitVideoSummary();
          continue;
        }
        if (frame.frameId % _frameLogSampleEvery == 0) {
          print(
            '[video] decoded frame_id=${frame.frameId} type=${pendingFrame.messageType} receive_ms=${frame.receivedTimestampMs} capture_ms=${frame.captureTimestampMs} queue_age_ms=${frame.queueAgeMs} host_clock_age_ms=${frame.hostClockAgeMs} decode_ms=${frame.decodeMs} receive_to_decode_start_ms=${frame.receiveToDecodeStartMs} width=${frame.width} height=${frame.height} rgba=${frame.rgbaBytes.length} expected=$expectedBytes placeholder=${frame.isPlaceholder}',
          );
        }
        _maybeEmitVideoSummary();
        if (!_videoFrames.isClosed) {
          _videoFrames.add(frame);
        }
      }
    } finally {
      _decodeLoopRunning = false;
      if (_pendingCompressedFrame != null) {
        unawaited(_runDecodeLoop());
      }
    }
  }

  Future<void> _ensureFrameDecodeIsolate() async {
    if (_frameDecodeSendPort != null) {
      return;
    }
    final readyPort = ReceivePort();
    final isolate = await Isolate.spawn(
      _frameDecodeIsolateMain,
      readyPort.sendPort,
    );
    final sendPort = await readyPort.first as SendPort;
    readyPort.close();
    final receivePort = ReceivePort();
    sendPort.send(<String, Object?>{
      'type': 'bind_reply',
      'reply_port': receivePort.sendPort,
    });
    receivePort.listen((dynamic message) {
      if (message is! Map) {
        return;
      }
      final requestId = message['request_id'] as int?;
      if (requestId == null) {
        return;
      }
      _pendingDecodeResponses
          .remove(requestId)
          ?.complete(message.cast<String, Object?>());
    });
    _frameDecodeIsolate = isolate;
    _frameDecodeReceivePort = receivePort;
    _frameDecodeSendPort = sendPort;
  }

  Future<Map<String, Object?>> _decodeFrameOnWorker(
    _PendingCompressedFrame frame,
  ) async {
    await _ensureFrameDecodeIsolate();
    final requestId = _nextDecodeRequestId++;
    final completer = Completer<Map<String, Object?>>();
    _pendingDecodeResponses[requestId] = completer;
    _frameDecodeSendPort!.send(<String, Object?>{
      'type': 'decode',
      'request_id': requestId,
      'message_type': frame.messageType,
      'received_timestamp_ms': frame.receivedTimestampMs,
      'payload': TransferableTypedData.fromList(<TypedData>[frame.payload]),
    });
    return completer.future;
  }

  Future<void> sendSettings({
    required int targetWidth,
    required int fps,
    required int jpegQuality,
    required bool viewOnly,
    int monitorIndex = 0,
    String clipboardMode = 'manual',
    bool deltaStreamEnabled = true,
    bool audioEnabled = false,
  }) async {
    _targetFrameBudgetMs = max(1, 1000 ~/ max(1, fps));
    await _sendControlJson(<String, Object?>{
      'type': 'settings',
      'target_width': targetWidth,
      'fps': fps,
      'jpeg_quality': jpegQuality,
      'view_only': viewOnly,
      'monitor_index': monitorIndex,
      'clipboard_mode': clipboardMode,
      'delta_stream_enabled': deltaStreamEnabled,
      'audio_enabled':
          (audioEnabled || kDebugAudioOnlyMode) && !kDebugAudioDisabled,
    });
  }

  Future<String?> getLocalClipboard() async {
    final data = await Clipboard.getData('text/plain');
    return data?.text;
  }

  Future<void> setLocalClipboard(String text) async {
    await Clipboard.setData(ClipboardData(text: text));
  }

  Future<void> sendClipboardText(String text) async {
    final syncId = _generateClipboardSyncId('client');
    _rememberClipboardSyncId(syncId);
    await _sendControlJson(<String, Object?>{
      'type': 'clipboard_set',
      'text': text,
      'sync_id': syncId,
      'source': 'client',
    });
  }

  Future<void> requestClipboardText() async {
    await _sendControlJson(<String, Object?>{'type': 'clipboard_get'});
  }

  void updateClipboardMode(String mode) {
    _clipboardMode = mode;
  }

  Future<void> sendClipboardMode(String mode) async {
    _clipboardMode = mode;
    await _sendControlJson(<String, Object?>{
      'type': 'clipboard_mode',
      'mode': mode,
    });
  }

  Future<void> stopAudioOutput() async {
    await _audioSink.stop();
  }

  Future<void> requestResync() async {
    await _sendControlJson(<String, Object?>{'type': 'resync_request'});
  }

  Future<void> sendFileBytes(
    String filename,
    Uint8List data, {
    void Function(double progress)? onProgress,
    bool Function()? shouldCancel,
  }) async {
    final digest = sha256.convert(data).toString();
    final transferId =
        'tx-${DateTime.now().microsecondsSinceEpoch}-${Random.secure().nextInt(1 << 30)}';
    _activeTransferId = transferId;
    const int chunkSize = 8 * 1024;
    final totalChunks = (data.length / chunkSize).ceil();
    print(
      'file transfer start transfer_id=$transferId filename=$filename size=${data.length} sha256=$digest chunk_size=$chunkSize total_chunks=$totalChunks',
    );
    final startAck = Completer<void>();
    _pendingTransferStarts[transferId] = startAck;
    await _sendControlJson(<String, Object?>{
      'type': 'file_start',
      'transfer_id': transferId,
      'filename': filename,
      'size': data.length,
      'sha256': digest,
    });
    await startAck.future.timeout(
      const Duration(seconds: 10),
      onTimeout: () {
        _pendingTransferStarts.remove(transferId);
        throw const SocketException(
          'Timed out waiting for host transfer start ack',
        );
      },
    );

    var seq = 0;
    for (var i = 0; i < data.length; i += chunkSize) {
      if (shouldCancel?.call() ?? false) {
        await sendFileCancel();
        throw const SocketException('File transfer cancelled');
      }
      final end = min(i + chunkSize, data.length);
      final chunk = data.sublist(i, end);
      final encoded = base64Encode(chunk);
      print(
        'file chunk transfer_id=$transferId seq=$seq offset=$i raw_len=${chunk.length} encoded_len=${encoded.length}',
      );
      final chunkAck = Completer<void>();
      final ackMap = _pendingTransferChunkAcks.putIfAbsent(
        transferId,
        () => <int, Completer<void>>{},
      );
      ackMap[seq] = chunkAck;
      const maxChunkAttempts = 3;
      var acked = false;
      for (var attempt = 1; attempt <= maxChunkAttempts; attempt++) {
        if (attempt > 1) {
          print(
            'retry file chunk transfer_id=$transferId seq=$seq attempt=$attempt',
          );
        }
        await _sendControlJson(<String, Object?>{
          'type': 'file_chunk',
          'transfer_id': transferId,
          'seq': seq,
          'offset': i,
          'data': encoded,
        });
        try {
          await chunkAck.future.timeout(const Duration(seconds: 4));
          acked = true;
          break;
        } on TimeoutException {
          if (attempt == maxChunkAttempts) {
            _pendingTransferChunkAcks[transferId]?.remove(seq);
            throw SocketException(
              'Timed out waiting for host chunk ack seq=$seq',
            );
          }
        }
      }
      if (!acked) {
        _pendingTransferChunkAcks[transferId]?.remove(seq);
        throw SocketException('Timed out waiting for host chunk ack seq=$seq');
      }
      onProgress?.call(end / data.length);
      seq += 1;
    }
    await _sendControlJson(<String, Object?>{
      'type': 'file_finish',
      'transfer_id': transferId,
      'chunk_count': totalChunks,
    });
    print(
      'file transfer finish transfer_id=$transferId total_bytes_sent=${data.length} total_chunks_sent=$totalChunks declared_sha256=$digest',
    );
    _activeTransferId = null;
  }

  Future<void> sendFileCancel() async {
    await _sendControlJson(<String, Object?>{
      'type': 'file_cancel',
      'transfer_id': _activeTransferId,
    });
    if (_activeTransferId != null) {
      _failPendingTransfer(
        _activeTransferId!,
        const SocketException('cancelled'),
      );
    }
    _activeTransferId = null;
  }

  Future<void> sendMouseMove(double relX, double relY) async {
    final x = relX.clamp(0.0, 1.0);
    final y = relY.clamp(0.0, 1.0);
    print(
      'sending mouse_move rel=(${x.toStringAsFixed(3)}, ${y.toStringAsFixed(3)})',
    );
    await _sendControlJson(<String, Object?>{
      'type': 'mouse_move',
      'x': x,
      'y': y,
    });
  }

  Future<void> sendMouseScroll(int delta) async {
    await _sendControlJson(<String, Object?>{
      'type': 'mouse_scroll',
      'delta': delta,
    });
  }

  Future<void> sendLeftClick() async {
    print('sending left_click');
    await _sendControlJson(<String, Object?>{'type': 'left_click'});
  }

  Future<void> sendRightClick() async {
    print('sending right_click');
    await _sendControlJson(<String, Object?>{'type': 'right_click'});
  }

  Future<void> sendKeyDown(int vk) async {
    await _sendControlJson(<String, Object?>{'type': 'key_down', 'vk': vk});
  }

  Future<void> sendKeyUp(int vk) async {
    await _sendControlJson(<String, Object?>{'type': 'key_up', 'vk': vk});
  }

  Future<void> sendKeyCombo(List<int> keys) async {
    for (final key in keys) {
      await sendKeyDown(key);
    }
    for (final key in keys.reversed) {
      await sendKeyUp(key);
    }
  }

  Future<(int, Uint8List)?> _readMessage() async {
    final typeBytes = await _readExact(1);
    if (typeBytes.isEmpty) {
      return null;
    }
    final messageType = typeBytes[0];

    final lenBytes = await _readExact(4);
    if (lenBytes.isEmpty) {
      return null;
    }
    final payloadLength = ByteData.sublistView(
      lenBytes,
    ).getUint32(0, Endian.big);

    final payload = await _readExact(payloadLength);
    if (payload.isEmpty && payloadLength > 0) {
      return null;
    }
    if (messageType == _msgAudioPacket ||
        messageType == _msgVideoKeyframe ||
        messageType == _msgVideoDelta ||
        messageType == _msgControlInput) {
      print('[rx] packet type=$messageType len=$payloadLength');
    }
    return (messageType, payload);
  }

  Future<Uint8List> _readExact(int length) async {
    if (length == 0) {
      return Uint8List(0);
    }

    final out = Uint8List(length);
    var written = 0;

    while (written < length) {
      if (_pendingOffset < _pending.length) {
        final available = _pending.length - _pendingOffset;
        final take = min(length - written, available);
        out.setRange(written, written + take, _pending, _pendingOffset);
        written += take;
        _pendingOffset += take;
        if (_pendingOffset >= _pending.length) {
          _pending = Uint8List(0);
          _pendingOffset = 0;
        }
        continue;
      }

      final iterator = _iterator;
      if (iterator == null) {
        throw const SocketException('Not connected');
      }

      final hasNext = await iterator.moveNext();
      if (!hasNext) {
        throw const SocketException(
          'Connection closed before enough data was received',
        );
      }
      _pending = iterator.current;
      _pendingOffset = 0;
    }

    return out;
  }

  int _peekFrameId(int messageType, Uint8List payload) {
    if ((messageType == _msgVideoKeyframe || messageType == _msgVideoDelta) &&
        payload.length >= 4) {
      return ByteData.sublistView(payload).getUint32(0, Endian.big);
    }
    return 0;
  }

  void _handleAudioPacket(Uint8List payload) {
    if (kDebugAudioDisabled) {
      return;
    }
    if (payload.length < 23) {
      return;
    }
    final data = ByteData.sublistView(payload);
    final channels = data.getUint16(12, Endian.big);
    final sampleRate = data.getUint32(14, Endian.big);
    final codec = data.getUint8(18);
    final audioLen = data.getUint32(19, Endian.big);
    final ptsMs = data.getUint64(0, Endian.big).toInt();
    final sequence = data.getUint32(8, Endian.big);
    if (payload.length < 23 + audioLen) {
      return;
    }
    if (codec != _audioCodecPcmS16Le) {
      _controlMessages.add(<String, dynamic>{
        'type': 'client_audio_error',
        'message': 'Unsupported audio codec: $codec',
      });
      return;
    }
    _audioPacketsReceived += 1;
    final receivedTimestampMs = DateTime.now().millisecondsSinceEpoch;
    final queueDepthAtEnqueue = _pendingAudioPackets.length + 1;
    _audioQueueDepthStats.add(queueDepthAtEnqueue);
    print(
      '[audio] packet received count=$_audioPacketsReceived seq=$sequence pts=$ptsMs sampleRate=$sampleRate channels=$channels codec=$codec bytes=$audioLen',
    );
    if (_pendingAudioPackets.length >= _maxPendingAudioPackets) {
      final dropped = _pendingAudioPackets.removeFirst();
      _audioPacketsDropped += 1;
      _audioLateDrops += 1;
      print(
        '[audio] late-drop seq=${dropped.sequence} queueAge=${dropped.queueAgeMs}ms totalDropped=$_audioPacketsDropped',
      );
    }
    _pendingAudioPackets.addLast(
      _PendingAudioPacket(
        payload: Uint8List.fromList(payload),
        receivedTimestampMs: receivedTimestampMs,
        enqueueTimestampMs: receivedTimestampMs,
        ptsMs: ptsMs,
        sequence: sequence,
        queueDepthAtEnqueue: queueDepthAtEnqueue,
      ),
    );
    _trimAudioQueueToLatencyBudget();
    if (!_audioDispatchRunning) {
      unawaited(_drainAudioQueue());
    }
  }

  Future<void> _drainAudioQueue() async {
    if (_audioDispatchRunning) {
      return;
    }
    _audioDispatchRunning = true;
    try {
      while (_pendingAudioPackets.isNotEmpty) {
        final pending = _pendingAudioPackets.removeFirst();
        final payload = pending.payload;
        final data = ByteData.sublistView(payload);
        final channels = data.getUint16(12, Endian.big);
        final sampleRate = data.getUint32(14, Endian.big);
        final audioLen = data.getUint32(19, Endian.big);
        final audioBytes = Uint8List.sublistView(payload, 23, 23 + audioLen);
        if (pending.queueAgeMs > _audioMaxQueueAgeMs) {
          _audioPacketsDropped += 1;
          _audioLateDrops += 1;
          print(
            '[audio] stale-drop seq=${pending.sequence} queueAge=${pending.queueAgeMs}ms totalDropped=$_audioPacketsDropped',
          );
          _maybeEmitAudioSummary();
          continue;
        }
        try {
          final submitStartMs = DateTime.now().millisecondsSinceEpoch;
          await _audioSink.playPcm16(
            sampleRate: sampleRate,
            channels: channels,
            data: audioBytes,
          );
          final submitEndMs = DateTime.now().millisecondsSinceEpoch;
          _audioPacketsSubmitted += 1;
          _audioReceiveToSubmitMs.add(
            submitEndMs - pending.receivedTimestampMs,
          );
          if (_audioPacketsSubmitted % _frameLogSampleEvery == 0) {
            final playbackStats = await _audioSink.getPlaybackStats();
            final occupancyMs = (playbackStats['buffer_occupancy_ms'] as num?)
                ?.toInt();
            if (occupancyMs != null) {
              _audioBufferOccupancyMs.add(occupancyMs);
            }
          }
          if (_audioPacketsSubmitted % _frameLogSampleEvery == 0) {
            print(
              '[audio] playback submitted count=$_audioPacketsSubmitted seq=${pending.sequence} pts=${pending.ptsMs} sampleRate=$sampleRate channels=$channels bytes=$audioLen queue=${_pendingAudioPackets.length} enqueue_depth=${pending.queueDepthAtEnqueue} submit_ms=${submitEndMs - submitStartMs} dropped=$_audioPacketsDropped',
            );
          }
          _maybeEmitAudioSummary();
        } catch (err) {
          _controlMessages.add(<String, dynamic>{
            'type': 'client_audio_error',
            'message': err.toString(),
          });
          print('[audio] playback error $err');
        }
      }
    } finally {
      _audioDispatchRunning = false;
      if (_pendingAudioPackets.isNotEmpty) {
        unawaited(_drainAudioQueue());
      }
    }
  }

  void _enqueueControlMessage(Map<String, dynamic> message) {
    _pendingControlMessages.addLast(message);
    if (!_controlDispatchRunning) {
      unawaited(_drainControlQueue());
    }
  }

  Future<void> _drainControlQueue() async {
    if (_controlDispatchRunning) {
      return;
    }
    _controlDispatchRunning = true;
    try {
      while (_pendingControlMessages.isNotEmpty) {
        final next = _pendingControlMessages.removeFirst();
        await _handleControlMessage(next);
      }
    } finally {
      _controlDispatchRunning = false;
      if (_pendingControlMessages.isNotEmpty) {
        unawaited(_drainControlQueue());
      }
    }
  }

  void _recordTransportDispatch(int dispatchStartMs) {
    final nowMs = DateTime.now().millisecondsSinceEpoch;
    _transportDispatchMs.add(nowMs - dispatchStartMs);
    _maybeEmitTransportSummary();
  }

  void _trimAudioQueueToLatencyBudget() {
    while (_pendingAudioPackets.isNotEmpty &&
        _pendingAudioPackets.first.queueAgeMs > _audioMaxQueueAgeMs) {
      final dropped = _pendingAudioPackets.removeFirst();
      _audioPacketsDropped += 1;
      _audioLateDrops += 1;
      print(
        '[audio] age-trim drop seq=${dropped.sequence} queueAge=${dropped.queueAgeMs}ms totalDropped=$_audioPacketsDropped',
      );
    }
  }

  Map<String, Object?> _buildDecodeBypassFrame(_PendingCompressedFrame frame) {
    _videoFramesDecodeBypassed += 1;
    return <String, Object?>{
      'frame_id': frame.frameId,
      'width': 2,
      'height': 2,
      'capture_timestamp_ms': _peekCaptureTimestampMs(
        frame.messageType,
        frame.payload,
      ),
      'received_timestamp_ms': frame.receivedTimestampMs,
      'rgba': TransferableTypedData.fromList(<TypedData>[
        Uint8List.fromList(<int>[
          24,
          24,
          24,
          255,
          24,
          24,
          24,
          255,
          24,
          24,
          24,
          255,
          24,
          24,
          24,
          255,
        ]),
      ]),
      'is_placeholder': true,
    };
  }

  int _peekCaptureTimestampMs(int messageType, Uint8List payload) {
    if (messageType == _msgVideoKeyframe && payload.length >= 12) {
      return ByteData.sublistView(payload).getUint64(4, Endian.big).toInt();
    }
    if (messageType == _msgVideoDelta && payload.length >= 16) {
      return ByteData.sublistView(payload).getUint64(8, Endian.big).toInt();
    }
    return DateTime.now().millisecondsSinceEpoch;
  }

  void _maybeEmitTransportSummary() {
    final nowMs = DateTime.now().millisecondsSinceEpoch;
    if (nowMs - _lastTransportSummaryAtMs < _latencySummaryIntervalMs ||
        _transportDispatchMs.isEmpty) {
      return;
    }
    _lastTransportSummaryAtMs = nowMs;
    _controlMessages.add(<String, dynamic>{
      'type': 'client_transport_latency_summary',
      'dispatch_avg_ms': _transportDispatchMs.averageLabel,
      'dispatch_p95_ms': _transportDispatchMs.p95,
    });
  }

  void _maybeEmitVideoSummary() {
    final nowMs = DateTime.now().millisecondsSinceEpoch;
    if (nowMs - _lastVideoSummaryAtMs < _latencySummaryIntervalMs ||
        _videoDecodeMs.isEmpty) {
      return;
    }
    _lastVideoSummaryAtMs = nowMs;
    _controlMessages.add(<String, dynamic>{
      'type': 'client_video_latency_summary',
      'receive_to_decode_start_avg_ms':
          _videoReceiveToDecodeStartMs.averageLabel,
      'receive_to_decode_start_p95_ms': _videoReceiveToDecodeStartMs.p95,
      'decode_avg_ms': _videoDecodeMs.averageLabel,
      'decode_p95_ms': _videoDecodeMs.p95,
      'receive_to_emit_avg_ms': _videoReceiveToEmitMs.averageLabel,
      'receive_to_emit_p95_ms': _videoReceiveToEmitMs.p95,
      'frames_replaced_before_decode': _replacedCompressedFrameCount,
      'stale_drops_after_decode': _staleDropsAfterDecode,
      'decode_bypass_frames': _videoFramesDecodeBypassed,
      'decode_pressure_drops': _decodePressureDrops,
      'input_to_visible_avg_ms': _inputToVisibleMs.averageLabel,
      'input_to_visible_p95_ms': _inputToVisibleMs.p95,
    });
  }

  void _maybeEmitAudioSummary() {
    final nowMs = DateTime.now().millisecondsSinceEpoch;
    if (nowMs - _lastAudioSummaryAtMs < _latencySummaryIntervalMs ||
        _audioReceiveToSubmitMs.isEmpty) {
      return;
    }
    _lastAudioSummaryAtMs = nowMs;
    _controlMessages.add(<String, dynamic>{
      'type': 'client_audio_latency_summary',
      'receive_to_audio_submit_avg_ms': _audioReceiveToSubmitMs.averageLabel,
      'receive_to_audio_submit_p95_ms': _audioReceiveToSubmitMs.p95,
      'audio_queue_depth_avg': _audioQueueDepthStats.averageLabel,
      'audio_queue_depth_p95': _audioQueueDepthStats.p95,
      'audio_late_drop_count': _audioLateDrops,
      'audio_buffer_occupancy_avg_ms': _audioBufferOccupancyMs.averageLabel,
      'audio_buffer_occupancy_p95_ms': _audioBufferOccupancyMs.p95,
      'audio_queue_target_ms': _audioMaxQueueAgeMs,
    });
  }

  bool _shouldDropDeltaUnderDecodePressure(int messageType) {
    if (messageType != _msgVideoDelta) {
      return false;
    }
    if (_videoDecodeMs.isEmpty) {
      return false;
    }
    final decodeAvgMs = _videoDecodeMs.average;
    final decodeP95Ms = _videoDecodeMs.p95;
    return decodeAvgMs > (_targetFrameBudgetMs * 1.1) ||
        decodeP95Ms > (_targetFrameBudgetMs * 1.35);
  }

  void markInputEventSent(String kind) {
    _pendingInputLatencyProbe = _InputLatencyProbe(
      kind: kind,
      sentAt: DateTime.now(),
      frameIdBeforeSend: _lastDecodedFrameId,
    );
  }

  Map<String, Object?>? takeCompletedInputLatency(RemoteVideoFrame frame) {
    final probe = _pendingInputLatencyProbe;
    if (probe == null || frame.frameId <= probe.frameIdBeforeSend) {
      return null;
    }
    _pendingInputLatencyProbe = null;
    final latencyMs = DateTime.now().difference(probe.sentAt).inMilliseconds;
    _inputToVisibleMs.add(latencyMs);
    return <String, Object?>{
      'kind': probe.kind,
      'latency_ms': latencyMs,
      'frame_id': frame.frameId,
      'avg_ms': _inputToVisibleMs.averageLabel,
      'p95_ms': _inputToVisibleMs.p95,
    };
  }

  Future<void> _handleControlMessage(Map<String, dynamic> message) async {
    final type = message['type'] as String?;

    if (type == 'file_transfer_started') {
      final transferId = message['transfer_id'] as String?;
      if (transferId != null) {
        _pendingTransferStarts.remove(transferId)?.complete();
      }
      _controlMessages.add(message);
      return;
    }

    if (type == 'file_chunk_ack') {
      final transferId = message['transfer_id'] as String?;
      final seq = message['seq'] as int?;
      if (transferId != null && seq != null) {
        _pendingTransferChunkAcks[transferId]?.remove(seq)?.complete();
        final ackMap = _pendingTransferChunkAcks[transferId];
        if (ackMap != null && ackMap.isEmpty) {
          _pendingTransferChunkAcks.remove(transferId);
        }
      }
      return;
    }

    if (type == 'file_transfer_result') {
      final transferId = message['transfer_id'] as String?;
      if (transferId != null) {
        if (message['success'] == true) {
          _pendingTransferStarts.remove(transferId);
          _pendingTransferChunkAcks.remove(transferId);
          if (_activeTransferId == transferId) {
            _activeTransferId = null;
          }
        } else {
          _failPendingTransfer(
            transferId,
            SocketException(
              message['error'] as String? ?? 'file transfer failed',
            ),
          );
          if (_activeTransferId == transferId) {
            _activeTransferId = null;
          }
        }
      }
      _controlMessages.add(message);
      return;
    }

    if (type == 'pair_challenge') {
      final identity = await getOrCreateDeviceIdentity();
      final pendingClientNonce = _pendingPairClientNonce;
      final deviceId = message['device_id'] as String? ?? '';
      final hostNonceB64 = message['host_nonce_b64'] as String?;
      final challengeB64 = message['challenge_b64'] as String?;
      if (pendingClientNonce == null ||
          deviceId != identity.deviceId ||
          hostNonceB64 == null ||
          challengeB64 == null) {
        _controlMessages.add(<String, dynamic>{
          'type': 'pair_result',
          'ok': false,
          'message': 'invalid_pair_challenge',
        });
        return;
      }
      final payload = _buildSignedPayload('AETHERLINK_PAIR_V1', <Uint8List>[
        Uint8List.fromList(utf8.encode(identity.deviceId)),
        pendingClientNonce,
        base64Decode(hostNonceB64),
        base64Decode(challengeB64),
      ]);
      final signature = await _trustIdentityService.signChallenge(payload);
      await _sendControlJson(<String, Object?>{
        'type': 'pair_proof',
        'device_id': identity.deviceId,
        'signature_b64': base64Encode(signature),
      });
      _controlMessages.add(<String, dynamic>{
        'type': 'pair_proof_sent',
        'device_id': identity.deviceId,
      });
      return;
    }

    if (type == 'trusted_auth_challenge') {
      if (!_trustedAuthEnabled) {
        _controlMessages.add(message);
        return;
      }
      final identity = await getOrCreateDeviceIdentity();
      final nonceB64 = message['nonce_b64'] as String?;
      final sessionContext = message['session_context'] as String?;
      if (nonceB64 == null || sessionContext == null) {
        _controlMessages.add(<String, dynamic>{
          'type': 'trusted_auth_result',
          'ok': false,
          'message': 'invalid_trusted_auth_challenge',
        });
        return;
      }
      final payload = _buildSignedPayload('AETHERLINK_AUTH_V1', <Uint8List>[
        Uint8List.fromList(utf8.encode(identity.deviceId)),
        base64Decode(nonceB64),
        Uint8List.fromList(utf8.encode(sessionContext)),
      ]);
      final signature = await _trustIdentityService.signChallenge(payload);
      await _sendControlJson(<String, Object?>{
        'type': 'trusted_auth',
        'device_id': identity.deviceId,
        'nonce_b64': nonceB64,
        'session_context': sessionContext,
        'signature_b64': base64Encode(signature),
      });
      _controlMessages.add(<String, dynamic>{
        'type': 'trusted_auth_sent',
        'device_id': identity.deviceId,
      });
      return;
    }

    if (type == 'clipboard_data') {
      final syncId = message['sync_id'] as String?;
      if (syncId != null && _hasRecentClipboardSyncId(syncId)) {
        return;
      }
      final text = message['text'] as String? ?? '';
      final textHash = sha256.convert(utf8.encode(text)).toString();
      final withinSuppressionWindow =
          _lastAppliedClipboardAt != null &&
          DateTime.now().difference(_lastAppliedClipboardAt!) <
              const Duration(seconds: 2);
      if (_lastAppliedClipboardHash == textHash && withinSuppressionWindow) {
        return;
      }
      if (syncId != null) {
        _rememberClipboardSyncId(syncId);
      }
      var appliedLocally = false;
      if (_clipboardMode == 'host_to_client') {
        await setLocalClipboard(text);
        _lastAppliedClipboardHash = textHash;
        _lastAppliedClipboardAt = DateTime.now();
        appliedLocally = true;
      }
      message = <String, dynamic>{
        ...message,
        'applied_locally': appliedLocally,
      };
    }
    _controlMessages.add(message);
  }

  void _failPendingTransfer(String transferId, Object error) {
    final startAck = _pendingTransferStarts.remove(transferId);
    if (startAck != null && !startAck.isCompleted) {
      startAck.completeError(error);
    }
    final chunkAcks = _pendingTransferChunkAcks.remove(transferId);
    if (chunkAcks != null) {
      for (final completer in chunkAcks.values) {
        if (!completer.isCompleted) {
          completer.completeError(error);
        }
      }
    }
  }

  Uint8List _buildSignedPayload(String domain, List<Uint8List> parts) {
    final builder = BytesBuilder(copy: false);
    builder.add(utf8.encode(domain));
    for (final part in parts) {
      final length = ByteData(4)..setUint32(0, part.length, Endian.big);
      builder.add(length.buffer.asUint8List());
      builder.add(part);
    }
    return builder.toBytes();
  }

  Uint8List _randomBytes(int length) {
    final random = Random.secure();
    return Uint8List.fromList(
      List<int>.generate(length, (_) => random.nextInt(256)),
    );
  }

  bool _hasRecentClipboardSyncId(String syncId) {
    return _recentClipboardSyncIds.contains(syncId);
  }

  void _rememberClipboardSyncId(String syncId) {
    _recentClipboardSyncIds.addLast(syncId);
    while (_recentClipboardSyncIds.length > 32) {
      _recentClipboardSyncIds.removeFirst();
    }
  }

  String _generateClipboardSyncId(String source) {
    return '$source-${DateTime.now().microsecondsSinceEpoch}';
  }

  Future<void> _sendControlJson(
    Map<String, Object?> message, {
    bool allowUnauthenticated = false,
  }) async {
    final socket = _socket;
    if (socket == null) {
      throw const SocketException('Not connected');
    }
    if (!_authenticated && !allowUnauthenticated && message['type'] != 'auth') {
      throw const SocketException('Not authenticated');
    }

    final sanitized = <String, Object>{};
    for (final entry in message.entries) {
      final value = entry.value;
      if (value != null) {
        sanitized[entry.key] = value;
      }
    }

    final payload = utf8.encode(jsonEncode(sanitized));
    final type = sanitized['type']?.toString() ?? 'unknown';
    print(
      'SEND control type=$type len=${payload.length} transport=${relayHostId == null ? 'direct' : 'relay'}',
    );
    final header = ByteData(5)
      ..setUint8(0, _msgControlInput)
      ..setUint32(1, payload.length, Endian.big);

    socket.add(header.buffer.asUint8List());
    socket.add(payload);
    await socket.flush();
  }

  bool _isPinnedCertificate(X509Certificate cert) {
    final certDigest = sha256.convert(cert.der).toString().toUpperCase();
    final pinned = _normalizeFingerprint(kPinnedServerSha256Fingerprint);
    return certDigest == pinned;
  }

  String _normalizeFingerprint(String fp) {
    return fp.replaceAll(':', '').replaceAll(' ', '').toUpperCase();
  }
}

void _frameDecodeIsolateMain(SendPort readyPort) {
  final commandPort = ReceivePort();
  readyPort.send(commandPort.sendPort);

  SendPort? replyPort;
  img.Image? frameBuffer;
  var currentFrameId = 0;

  commandPort.listen((dynamic rawMessage) {
    if (rawMessage is! Map) {
      return;
    }
    final message = rawMessage.cast<String, Object?>();
    final type = message['type'] as String?;
    if (type == 'bind_reply') {
      replyPort = message['reply_port'] as SendPort?;
      return;
    }
    if (type != 'decode' || replyPort == null) {
      return;
    }

    final requestId = message['request_id'] as int;
    final messageType = message['message_type'] as int;
    final receivedTimestampMs = message['received_timestamp_ms'] as int;
    final payload = (message['payload'] as TransferableTypedData)
        .materialize()
        .asUint8List();
    final response = _decodeFramePayloadInIsolate(
      frameBuffer: frameBuffer,
      currentFrameId: currentFrameId,
      messageType: messageType,
      payload: payload,
      receivedTimestampMs: receivedTimestampMs,
    );

    if (response case {'frame_buffer': final img.Image nextFrameBuffer}) {
      frameBuffer = nextFrameBuffer;
      currentFrameId = response['frame_id'] as int;
    }

    replyPort!.send(<String, Object?>{
      'request_id': requestId,
      ...response..remove('frame_buffer'),
    });
  });
}

Map<String, Object?> _decodeFramePayloadInIsolate({
  required img.Image? frameBuffer,
  required int currentFrameId,
  required int messageType,
  required Uint8List payload,
  required int receivedTimestampMs,
}) {
  if (messageType == _msgVideoFrame) {
    return <String, Object?>{};
  }
  if (messageType == _msgVideoKeyframe) {
    return _decodeKeyframeInIsolate(payload, receivedTimestampMs);
  }
  if (messageType == _msgVideoDelta) {
    return _decodeDeltaInIsolate(
      frameBuffer: frameBuffer,
      currentFrameId: currentFrameId,
      payload: payload,
      receivedTimestampMs: receivedTimestampMs,
    );
  }
  return <String, Object?>{};
}

Map<String, Object?> _decodeKeyframeInIsolate(
  Uint8List payload,
  int receivedTimestampMs,
) {
  final data = ByteData.sublistView(payload);
  if (payload.length < 25) {
    return <String, Object?>{'drop_reason': 'short_keyframe'};
  }
  final frameId = data.getUint32(0, Endian.big);
  final captureTimestampMs = data.getUint64(4, Endian.big);
  final width = data.getUint32(12, Endian.big);
  final height = data.getUint32(16, Endian.big);
  final codec = data.getUint8(20);
  final imageLen = data.getUint32(21, Endian.big);
  if (payload.length < 25 + imageLen) {
    return <String, Object?>{
      'frame_id': frameId,
      'drop_reason': 'truncated_keyframe',
    };
  }
  final imageBytes = payload.sublist(25, 25 + imageLen);
  final decoded = _decodeImageInIsolate(codec, imageBytes);
  if (decoded == null) {
    return <String, Object?>{
      'frame_id': frameId,
      'drop_reason': 'decode_failed',
    };
  }
  if (decoded.width != width || decoded.height != height) {
    return <String, Object?>{
      'frame_id': frameId,
      'drop_reason': 'decoded_dimension_mismatch',
    };
  }
  final rgba = Uint8List.fromList(
    decoded.getBytes(order: img.ChannelOrder.rgba),
  );
  final expectedBytes = width * height * 4;
  if (rgba.length != expectedBytes) {
    return <String, Object?>{
      'frame_id': frameId,
      'drop_reason': 'rgba_length_mismatch',
    };
  }
  return <String, Object?>{
    'frame_id': frameId,
    'width': width,
    'height': height,
    'capture_timestamp_ms': captureTimestampMs,
    'received_timestamp_ms': receivedTimestampMs,
    'rgba': TransferableTypedData.fromList(<TypedData>[rgba]),
    'frame_buffer': decoded,
  };
}

Map<String, Object?> _decodeDeltaInIsolate({
  required img.Image? frameBuffer,
  required int currentFrameId,
  required Uint8List payload,
  required int receivedTimestampMs,
}) {
  final data = ByteData.sublistView(payload);
  if (payload.length < 28) {
    return <String, Object?>{'drop_reason': 'short_delta'};
  }
  final frameId = data.getUint32(0, Endian.big);
  final baseFrameId = data.getUint32(4, Endian.big);
  final captureTimestampMs = data.getUint64(8, Endian.big);
  final frameWidth = data.getUint32(16, Endian.big);
  final frameHeight = data.getUint32(20, Endian.big);
  final moveCount = data.getUint16(24, Endian.big);
  final patchCount = data.getUint16(26, Endian.big);

  if (frameBuffer == null ||
      currentFrameId != baseFrameId ||
      frameBuffer.width != frameWidth ||
      frameBuffer.height != frameHeight) {
    return <String, Object?>{
      'frame_id': frameId,
      'request_resync': true,
      'drop_reason': 'base_frame_mismatch',
    };
  }

  var offset = 28;
  for (var i = 0; i < moveCount; i += 1) {
    if (payload.length < offset + 24) {
      return <String, Object?>{
        'frame_id': frameId,
        'drop_reason': 'truncated_move',
      };
    }
    final srcX = data.getInt32(offset, Endian.big);
    final srcY = data.getInt32(offset + 4, Endian.big);
    final dstX = data.getInt32(offset + 8, Endian.big);
    final dstY = data.getInt32(offset + 12, Endian.big);
    final width = data.getUint32(offset + 16, Endian.big);
    final height = data.getUint32(offset + 20, Endian.big);
    if (!_rectWithinFrame(srcX, srcY, width, height, frameWidth, frameHeight) ||
        !_rectWithinFrame(dstX, dstY, width, height, frameWidth, frameHeight)) {
      return <String, Object?>{
        'frame_id': frameId,
        'request_resync': true,
        'drop_reason': 'move_bounds_mismatch',
      };
    }
    _applyMoveInIsolate(frameBuffer, srcX, srcY, dstX, dstY, width, height);
    offset += 24;
  }

  for (var i = 0; i < patchCount; i += 1) {
    if (payload.length < offset + 21) {
      return <String, Object?>{
        'frame_id': frameId,
        'drop_reason': 'truncated_patch_header',
      };
    }
    final x = data.getInt32(offset, Endian.big);
    final y = data.getInt32(offset + 4, Endian.big);
    final width = data.getUint32(offset + 8, Endian.big);
    final height = data.getUint32(offset + 12, Endian.big);
    final codec = data.getUint8(offset + 16);
    final imageLen = data.getUint32(offset + 17, Endian.big);
    offset += 21;
    if (!_rectWithinFrame(x, y, width, height, frameWidth, frameHeight)) {
      return <String, Object?>{
        'frame_id': frameId,
        'request_resync': true,
        'drop_reason': 'patch_bounds_mismatch',
      };
    }
    if (payload.length < offset + imageLen) {
      return <String, Object?>{
        'frame_id': frameId,
        'drop_reason': 'truncated_patch_data',
      };
    }
    final patchBytes = payload.sublist(offset, offset + imageLen);
    offset += imageLen;
    final patch = _decodeImageInIsolate(codec, patchBytes);
    if (patch == null) {
      continue;
    }
    if (patch.width <= 0 || patch.height <= 0) {
      continue;
    }
    _applyPatchInIsolate(frameBuffer, patch, x, y, width, height);
  }

  final rgba = Uint8List.fromList(
    frameBuffer.getBytes(order: img.ChannelOrder.rgba),
  );
  final expectedBytes = frameWidth * frameHeight * 4;
  if (rgba.length != expectedBytes) {
    return <String, Object?>{
      'frame_id': frameId,
      'request_resync': true,
      'drop_reason': 'delta_rgba_length_mismatch',
    };
  }

  return <String, Object?>{
    'frame_id': frameId,
    'width': frameWidth,
    'height': frameHeight,
    'capture_timestamp_ms': captureTimestampMs,
    'received_timestamp_ms': receivedTimestampMs,
    'rgba': TransferableTypedData.fromList(<TypedData>[rgba]),
    'frame_buffer': frameBuffer,
  };
}

img.Image? _decodeImageInIsolate(int codec, Uint8List bytes) {
  switch (codec) {
    case _codecJpeg:
      return img.decodeJpg(bytes);
    default:
      return null;
  }
}

void _applyPatchInIsolate(
  img.Image target,
  img.Image patch,
  int x,
  int y,
  int width,
  int height,
) {
  final normalizedPatch = (patch.width == width && patch.height == height)
      ? patch
      : img.copyResize(patch, width: width, height: height);
  img.compositeImage(target, normalizedPatch, dstX: x, dstY: y);
}

void _applyMoveInIsolate(
  img.Image target,
  int srcX,
  int srcY,
  int dstX,
  int dstY,
  int width,
  int height,
) {
  final copy = img.copyCrop(
    target,
    x: srcX,
    y: srcY,
    width: width,
    height: height,
  );
  img.compositeImage(target, copy, dstX: dstX, dstY: dstY);
}

bool _rectWithinFrame(
  int x,
  int y,
  int width,
  int height,
  int frameWidth,
  int frameHeight,
) {
  if (x < 0 || y < 0 || width <= 0 || height <= 0) {
    return false;
  }
  return x + width <= frameWidth && y + height <= frameHeight;
}
