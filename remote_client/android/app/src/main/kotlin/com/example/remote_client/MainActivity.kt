package com.example.remote_client

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioTrack
import android.os.Build
import android.provider.Settings
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.Signature
import java.security.spec.ECGenParameterSpec
import java.util.UUID
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

class MainActivity : FlutterActivity() {
    private val audioExecutor: ExecutorService = Executors.newSingleThreadExecutor()
    @Volatile
    private var audioTrack: AudioTrack? = null
    @Volatile
    private var activeSampleRate: Int? = null
    @Volatile
    private var activeChannels: Int? = null

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)

        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "aetherlink/audio")
            .setMethodCallHandler { call, result ->
                when (call.method) {
                    "startPcm16" -> {
                        val sampleRate = call.argument<Int>("sampleRate") ?: 48000
                        val channels = call.argument<Int>("channels") ?: 2
                        try {
                            startPcm16(sampleRate, channels)
                            result.success(null)
                        } catch (err: Exception) {
                            result.error("audio_start_failed", err.message, null)
                        }
                    }
                    "writePcm16" -> {
                        val data = call.argument<ByteArray>("data")
                        if (data == null) {
                            result.error("audio_write_failed", "Missing audio payload", null)
                            return@setMethodCallHandler
                        }
                        audioExecutor.execute {
                            try {
                                audioTrack?.write(data, 0, data.size, AudioTrack.WRITE_BLOCKING)
                            } catch (_: Exception) {
                            }
                        }
                        result.success(null)
                    }
                    "stopAudio" -> {
                        stopAudio()
                        result.success(null)
                    }
                    else -> result.notImplemented()
                }
            }

        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "aetherlink/trust")
            .setMethodCallHandler { call, result ->
                when (call.method) {
                    "getOrCreateDeviceIdentity" -> {
                        try {
                            result.success(getOrCreateDeviceIdentity())
                        } catch (err: Exception) {
                            result.error("trust_identity_failed", err.message, null)
                        }
                    }
                    "signChallenge" -> {
                        val payload = call.argument<ByteArray>("payload")
                        if (payload == null) {
                            result.error("trust_sign_failed", "Missing payload", null)
                            return@setMethodCallHandler
                        }
                        try {
                            result.success(signChallenge(payload))
                        } catch (err: Exception) {
                            result.error("trust_sign_failed", err.message, null)
                        }
                    }
                    "forgetLocalIdentity" -> {
                        try {
                            forgetLocalIdentity()
                            result.success(true)
                        } catch (err: Exception) {
                            result.error("trust_forget_failed", err.message, null)
                        }
                    }
                    else -> result.notImplemented()
                }
            }
    }

    private fun startPcm16(sampleRate: Int, channels: Int) {
        val channelConfig = if (channels == 1) AudioFormat.CHANNEL_OUT_MONO else AudioFormat.CHANNEL_OUT_STEREO
        val minBufferSize = AudioTrack.getMinBufferSize(
            sampleRate,
            channelConfig,
            AudioFormat.ENCODING_PCM_16BIT,
        )
        val bufferSize = if (minBufferSize > 0) minBufferSize * 4 else sampleRate * channels * 2

        val current = audioTrack
        if (current != null) {
            val sameConfig = activeSampleRate == sampleRate && activeChannels == channels
            if (sameConfig && current.state == AudioTrack.STATE_INITIALIZED) {
                if (current.playState != AudioTrack.PLAYSTATE_PLAYING) {
                    current.play()
                }
                return
            }
            stopAudio()
        }

        val track = AudioTrack(
            AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_MEDIA)
                .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                .build(),
            AudioFormat.Builder()
                .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                .setSampleRate(sampleRate)
                .setChannelMask(channelConfig)
                .build(),
            bufferSize,
            AudioTrack.MODE_STREAM,
            AudioManager.AUDIO_SESSION_ID_GENERATE,
        )
        if (track.state != AudioTrack.STATE_INITIALIZED) {
            track.release()
            throw IllegalStateException("AudioTrack initialization failed")
        }
        track.play()
        audioTrack = track
        activeSampleRate = sampleRate
        activeChannels = channels
    }

    private fun stopAudio() {
        val track = audioTrack ?: return
        audioTrack = null
        activeSampleRate = null
        activeChannels = null
        audioExecutor.execute {
            try {
                track.pause()
                track.flush()
            } catch (_: Exception) {
            }
            try {
                track.stop()
            } catch (_: Exception) {
            }
            track.release()
        }
    }

    private fun trustPrefs() = getSharedPreferences("aetherlink_trust", Context.MODE_PRIVATE)

    private fun getOrCreateDeviceIdentity(): Map<String, String> {
        ensureTrustKeyExists()
        val prefs = trustPrefs()
        val deviceId = prefs.getString("device_id", null) ?: UUID.randomUUID().toString().also {
            prefs.edit().putString("device_id", it).apply()
        }
        val alias = prefs.getString("keystore_alias", null) ?: TRUST_KEY_ALIAS.also {
            prefs.edit().putString("keystore_alias", it).apply()
        }
        val deviceName = prefs.getString("device_name", null) ?: defaultDeviceName().also {
            prefs.edit().putString("device_name", it).apply()
        }
        val publicKeyPem = exportPublicKeyPem(alias)
        return mapOf(
            "deviceId" to deviceId,
            "deviceName" to deviceName,
            "keystoreAlias" to alias,
            "publicKeyPem" to publicKeyPem,
        )
    }

    private fun signChallenge(payload: ByteArray): ByteArray {
        ensureTrustKeyExists()
        val keyStore = KeyStore.getInstance(ANDROID_KEY_STORE).apply { load(null) }
        val entry = keyStore.getEntry(TRUST_KEY_ALIAS, null) as? KeyStore.PrivateKeyEntry
            ?: throw IllegalStateException("Trust key entry missing")
        val signature = Signature.getInstance("SHA256withECDSA")
        signature.initSign(entry.privateKey)
        signature.update(payload)
        return signature.sign()
    }

    private fun forgetLocalIdentity() {
        val keyStore = KeyStore.getInstance(ANDROID_KEY_STORE).apply { load(null) }
        if (keyStore.containsAlias(TRUST_KEY_ALIAS)) {
            keyStore.deleteEntry(TRUST_KEY_ALIAS)
        }
        trustPrefs().edit().clear().apply()
    }

    private fun ensureTrustKeyExists() {
        val keyStore = KeyStore.getInstance(ANDROID_KEY_STORE).apply { load(null) }
        if (keyStore.containsAlias(TRUST_KEY_ALIAS)) {
            return
        }
        val generator = KeyPairGenerator.getInstance(
            KeyProperties.KEY_ALGORITHM_EC,
            ANDROID_KEY_STORE,
        )
        val spec = KeyGenParameterSpec.Builder(
            TRUST_KEY_ALIAS,
            KeyProperties.PURPOSE_SIGN or KeyProperties.PURPOSE_VERIFY,
        )
            .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
            .setDigests(KeyProperties.DIGEST_SHA256)
            .setUserAuthenticationRequired(false)
            .build()
        generator.initialize(spec)
        generator.generateKeyPair()
    }

    private fun exportPublicKeyPem(alias: String): String {
        val keyStore = KeyStore.getInstance(ANDROID_KEY_STORE).apply { load(null) }
        val cert = keyStore.getCertificate(alias) ?: throw IllegalStateException("Certificate missing for alias $alias")
        val b64 = Base64.encodeToString(cert.publicKey.encoded, Base64.NO_WRAP)
        return buildString {
            append("-----BEGIN PUBLIC KEY-----\n")
            var index = 0
            while (index < b64.length) {
                val end = (index + 64).coerceAtMost(b64.length)
                append(b64.substring(index, end)).append('\n')
                index = end
            }
            append("-----END PUBLIC KEY-----\n")
        }
    }

    private fun defaultDeviceName(): String {
        val androidId = Settings.Secure.getString(contentResolver, Settings.Secure.ANDROID_ID)
        val model = listOfNotNull(Build.MANUFACTURER, Build.MODEL).joinToString(" ").trim()
        return if (androidId.isNullOrBlank()) model.ifBlank { "Android Device" } else {
            "${model.ifBlank { "Android Device" }}-${androidId.takeLast(4)}"
        }
    }

    override fun onDestroy() {
        stopAudio()
        audioExecutor.shutdownNow()
        super.onDestroy()
    }

    companion object {
        private const val ANDROID_KEY_STORE = "AndroidKeyStore"
        private const val TRUST_KEY_ALIAS = "aetherlink_device_key"
    }
}
