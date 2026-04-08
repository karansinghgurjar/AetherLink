import 'dart:async';
import 'dart:convert';
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

  static final AndroidTrustIdentityService instance = AndroidTrustIdentityService._();
  static const MethodChannel _channel = MethodChannel('aetherlink/trust');

  Future<TrustedDeviceIdentity> getOrCreateDeviceIdentity() async {
    final raw = await _channel.invokeMapMethod<String, dynamic>('getOrCreateDeviceIdentity');
    if (raw == null) {
      throw PlatformException(code: 'trust_identity_failed', message: 'Missing identity payload');
    }
    return TrustedDeviceIdentity(
      deviceId: raw['deviceId'] as String? ?? '',
      deviceName: raw['deviceName'] as String? ?? 'Android Device',
      keystoreAlias: raw['keystoreAlias'] as String? ?? '',
      publicKeyPem: raw['publicKeyPem'] as String? ?? '',
    );
  }

  Future<Uint8List> signChallenge(Uint8List payload) async {
    final signature = await _channel.invokeMethod<Uint8List>('signChallenge', <String, Object>{
      'payload': payload,
    });
    if (signature == null || signature.isEmpty) {
      throw PlatformException(code: 'trust_sign_failed', message: 'Empty signature');
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

  Future<void> playPcm16({required int sampleRate, required int channels, required Uint8List data}) async {
    if (!_started || _sampleRate != sampleRate || _channels != channels) {
      await _channel.invokeMethod<void>('startPcm16', <String, Object>{
        'sampleRate': sampleRate,
        'channels': channels,
      });
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
    _started = false;
    _sampleRate = null;
    _channels = null;
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
  final StreamController<Map<String, dynamic>> _controlMessages = StreamController<Map<String, dynamic>>.broadcast();
  final ListQueue<String> _recentClipboardSyncIds = ListQueue<String>();
  String _clipboardMode = 'manual';
  String? _lastAppliedClipboardHash;
  DateTime? _lastAppliedClipboardAt;
  Uint8List _pending = Uint8List(0);
  int _pendingOffset = 0;
  bool _authenticated = false;
  img.Image? _frameBuffer;
  int _currentFrameId = 0;
  final _AndroidAudioSink _audioSink = _AndroidAudioSink();
  final AndroidTrustIdentityService _trustIdentityService = AndroidTrustIdentityService.instance;
  TrustedDeviceIdentity? _trustedIdentity;
  Uint8List? _pendingPairClientNonce;
  bool _trustedAuthEnabled = false;
  String? _activeTransferId;
  final Map<String, Completer<void>> _pendingTransferStarts = <String, Completer<void>>{};
  final Map<String, Map<int, Completer<void>>> _pendingTransferChunkAcks =
      <String, Map<int, Completer<void>>>{};

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
      onBadCertificate: (cert) => kAllowInsecureLocalTlsForTesting || _isPinnedCertificate(cert),
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

      final decoded = jsonDecode(utf8.decode(message.$2)) as Map<String, dynamic>;
      final type = decoded['type'] as String?;
      if (type == 'relay_session_ready') {
        _controlMessages.add(decoded);
        return;
      }
      if (type == 'relay_error') {
        throw SocketException(decoded['message'] as String? ?? 'Relay connection failed');
      }
      _controlMessages.add(decoded);
    }
  }

  Future<TrustedDeviceIdentity> getOrCreateDeviceIdentity() async {
    _trustedIdentity ??= await _trustIdentityService.getOrCreateDeviceIdentity();
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
      'device_name': (deviceName?.trim().isNotEmpty ?? false) ? deviceName!.trim() : identity.deviceName,
      'public_key_pem': identity.publicKeyPem,
      'client_nonce_b64': base64Encode(clientNonce),
    });
  }

  Future<void> close() async {
    for (final transferId in <String>{
      ..._pendingTransferStarts.keys,
      ..._pendingTransferChunkAcks.keys,
    }) {
      _failPendingTransfer(transferId, const SocketException('connection closed'));
    }
    await _iterator?.cancel();
    await _socket?.close();
    _iterator = null;
    _socket = null;
    _pending = Uint8List(0);
    _pendingOffset = 0;
    _authenticated = false;
    _frameBuffer = null;
    _currentFrameId = 0;
    _activeTransferId = null;
    _recentClipboardSyncIds.clear();
    _clipboardMode = 'manual';
    _lastAppliedClipboardHash = null;
    _lastAppliedClipboardAt = null;
    await _audioSink.stop();
  }

  Stream<Map<String, dynamic>> get controlMessages => _controlMessages.stream;

  Future<Uint8List?> fetchSingleFrame() async {
    await connect();

    while (true) {
      final message = await _readMessage();
      if (message == null) {
        return null;
      }

      final output = await _decodeFrameMessage(message.$1, message.$2);
      if (output != null) {
        return output;
      }
    }
  }

  Stream<Uint8List> streamFrames() async* {
    await connect();
    while (true) {
      final message = await _readMessage();
      if (message == null) {
        break;
      }

      final output = await _decodeFrameMessage(message.$1, message.$2);
      if (output != null) {
        yield output;
      }
    }
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
    await _sendControlJson(<String, Object?>{
      'type': 'settings',
      'target_width': targetWidth,
      'fps': fps,
      'jpeg_quality': jpegQuality,
      'view_only': viewOnly,
      'monitor_index': monitorIndex,
      'clipboard_mode': clipboardMode,
      'delta_stream_enabled': deltaStreamEnabled,
      'audio_enabled': audioEnabled,
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
    await _sendControlJson(<String, Object?>{'type': 'clipboard_mode', 'mode': mode});
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
    final transferId = 'tx-${DateTime.now().microsecondsSinceEpoch}-${Random.secure().nextInt(1 << 30)}';
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
        throw const SocketException('Timed out waiting for host transfer start ack');
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
      final ackMap =
          _pendingTransferChunkAcks.putIfAbsent(transferId, () => <int, Completer<void>>{});
      ackMap[seq] = chunkAck;
      const maxChunkAttempts = 3;
      var acked = false;
      for (var attempt = 1; attempt <= maxChunkAttempts; attempt++) {
        if (attempt > 1) {
          print('retry file chunk transfer_id=$transferId seq=$seq attempt=$attempt');
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
            throw SocketException('Timed out waiting for host chunk ack seq=$seq');
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
      _failPendingTransfer(_activeTransferId!, const SocketException('cancelled'));
    }
    _activeTransferId = null;
  }

  Future<void> sendMouseMove(double relX, double relY) async {
    final x = relX.clamp(0.0, 1.0);
    final y = relY.clamp(0.0, 1.0);
    print('sending mouse_move rel=(${x.toStringAsFixed(3)}, ${y.toStringAsFixed(3)})');
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
    final payloadLength = ByteData.sublistView(lenBytes).getUint32(0, Endian.big);

    final payload = await _readExact(payloadLength);
    if (payload.isEmpty && payloadLength > 0) {
      return null;
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
        out.setRange(
          written,
          written + take,
          _pending,
          _pendingOffset,
        );
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
        throw const SocketException('Connection closed before enough data was received');
      }
      _pending = iterator.current;
      _pendingOffset = 0;
    }

    return out;
  }

  Future<Uint8List?> _decodeFrameMessage(int messageType, Uint8List payload) async {
    if (messageType == _msgVideoFrame) {
      return payload;
    }
    if (messageType == _msgControlInput) {
      try {
        final decoded = jsonDecode(utf8.decode(payload)) as Map<String, dynamic>;
        await _handleControlMessage(decoded);
      } catch (_) {
        // Ignore malformed control messages from the host.
      }
      return null;
    }
    if (messageType == _msgVideoKeyframe) {
      return _handleKeyframe(payload);
    }
    if (messageType == _msgVideoDelta) {
      return _handleDelta(payload);
    }
    if (messageType == _msgAudioPacket) {
      await _handleAudioPacket(payload);
      return null;
    }
    return null;
  }

  Uint8List? _handleKeyframe(Uint8List payload) {
    final data = ByteData.sublistView(payload);
    if (payload.length < 17) {
      return null;
    }
    final frameId = data.getUint32(0, Endian.big);
    final codec = data.getUint8(12);
    final imageLen = data.getUint32(13, Endian.big);
    if (payload.length < 17 + imageLen) {
      return null;
    }
    final imageBytes = payload.sublist(17, 17 + imageLen);
    final decoded = _decodeImage(codec, imageBytes);
    if (decoded == null) {
      return null;
    }
    _frameBuffer = decoded;
    _currentFrameId = frameId;
    return Uint8List.fromList(img.encodePng(decoded));
  }

  Future<Uint8List?> _handleDelta(Uint8List payload) async {
    final data = ByteData.sublistView(payload);
    if (payload.length < 20) {
      return null;
    }
    final frameId = data.getUint32(0, Endian.big);
    final baseFrameId = data.getUint32(4, Endian.big);
    final moveCount = data.getUint16(16, Endian.big);
    final patchCount = data.getUint16(18, Endian.big);

    final frameBuffer = _frameBuffer;
    final frameWidth = data.getUint32(8, Endian.big);
    final frameHeight = data.getUint32(12, Endian.big);
    if (frameBuffer == null || _currentFrameId != baseFrameId || frameBuffer.width != frameWidth || frameBuffer.height != frameHeight) {
      await requestResync();
      return null;
    }

    var offset = 20;
    for (var i = 0; i < moveCount; i += 1) {
      if (payload.length < offset + 24) {
        return null;
      }
      final srcX = data.getInt32(offset, Endian.big);
      final srcY = data.getInt32(offset + 4, Endian.big);
      final dstX = data.getInt32(offset + 8, Endian.big);
      final dstY = data.getInt32(offset + 12, Endian.big);
      final width = data.getUint32(offset + 16, Endian.big);
      final height = data.getUint32(offset + 20, Endian.big);
      _applyMove(frameBuffer, srcX, srcY, dstX, dstY, width, height);
      offset += 24;
    }

    for (var i = 0; i < patchCount; i += 1) {
      if (payload.length < offset + 21) {
        return null;
      }
      final x = data.getInt32(offset, Endian.big);
      final y = data.getInt32(offset + 4, Endian.big);
      final width = data.getUint32(offset + 8, Endian.big);
      final height = data.getUint32(offset + 12, Endian.big);
      final codec = data.getUint8(offset + 16);
      final imageLen = data.getUint32(offset + 17, Endian.big);
      offset += 21;
      if (payload.length < offset + imageLen) {
        return null;
      }
      final patchBytes = payload.sublist(offset, offset + imageLen);
      offset += imageLen;
      final patch = _decodeImage(codec, patchBytes);
      if (patch == null) {
        continue;
      }
      _applyPatch(frameBuffer, patch, x, y, width, height);
    }

    _currentFrameId = frameId;
    return Uint8List.fromList(img.encodePng(frameBuffer));
  }


  Future<void> _handleAudioPacket(Uint8List payload) async {
    if (payload.length < 23) {
      return;
    }
    final data = ByteData.sublistView(payload);
    final channels = data.getUint16(12, Endian.big);
    final sampleRate = data.getUint32(14, Endian.big);
    final codec = data.getUint8(18);
    final audioLen = data.getUint32(19, Endian.big);
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
    final audioBytes = Uint8List.sublistView(payload, 23, 23 + audioLen);
    print(
      'audio packet received sampleRate=$sampleRate channels=$channels codec=$codec bytes=$audioLen',
    );
    try {
      await _audioSink.playPcm16(
        sampleRate: sampleRate,
        channels: channels,
        data: audioBytes,
      );
    } catch (err) {
      _controlMessages.add(<String, dynamic>{
        'type': 'client_audio_error',
        'message': err.toString(),
      });
    }
  }
  img.Image? _decodeImage(int codec, Uint8List bytes) {
    switch (codec) {
      case _codecJpeg:
        return img.decodeJpg(bytes);
      default:
        return null;
    }
  }

  void _applyPatch(img.Image target, img.Image patch, int x, int y, int width, int height) {
    final normalizedPatch = (patch.width == width && patch.height == height)
        ? patch
        : img.copyResize(patch, width: width, height: height);
    img.compositeImage(target, normalizedPatch, dstX: x, dstY: y);
  }

  void _applyMove(img.Image target, int srcX, int srcY, int dstX, int dstY, int width, int height) {
    final copy = img.copyCrop(target, x: srcX, y: srcY, width: width, height: height);
    img.compositeImage(target, copy, dstX: dstX, dstY: dstY);
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
            SocketException(message['error'] as String? ?? 'file transfer failed'),
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
      if (pendingClientNonce == null || deviceId != identity.deviceId || hostNonceB64 == null || challengeB64 == null) {
        _controlMessages.add(<String, dynamic>{
          'type': 'pair_result',
          'ok': false,
          'message': 'invalid_pair_challenge',
        });
        return;
      }
      final payload = _buildSignedPayload(
        'AETHERLINK_PAIR_V1',
        <Uint8List>[
          Uint8List.fromList(utf8.encode(identity.deviceId)),
          pendingClientNonce,
          base64Decode(hostNonceB64),
          base64Decode(challengeB64),
        ],
      );
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
      final payload = _buildSignedPayload(
        'AETHERLINK_AUTH_V1',
        <Uint8List>[
          Uint8List.fromList(utf8.encode(identity.deviceId)),
          base64Decode(nonceB64),
          Uint8List.fromList(utf8.encode(sessionContext)),
        ],
      );
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
      final withinSuppressionWindow = _lastAppliedClipboardAt != null &&
          DateTime.now().difference(_lastAppliedClipboardAt!) < const Duration(seconds: 2);
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
      message = <String, dynamic>{...message, 'applied_locally': appliedLocally};
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
    return Uint8List.fromList(List<int>.generate(length, (_) => random.nextInt(256)));
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
    print('SEND control type=$type len=${payload.length} transport=${relayHostId == null ? 'direct' : 'relay'}');
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









