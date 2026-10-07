package app.norishell.android.bridge

import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.security.keystore.StrongBoxUnavailableException
import java.security.KeyStore
import javax.crypto.AEADBadTagException
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * AES-256-GCM key in AndroidKeyStore that wraps the Vault auto-unlock device
 * key. No user authentication: the contract is "unlock at app start on this
 * device", equal to DPAPI/Keychain WhenUnlockedThisDeviceOnly. The blob layout
 * is `IV(12) || ciphertext || tag(16)`.
 *
 * Error codes: `keystore.key_missing` (alias absent, invalidated, or the blob
 * does not authenticate under the current key), `keystore.unavailable`
 * (AndroidKeyStore cannot be loaded), `keystore.failed` (anything else).
 */
object DeviceKeyStore {
    private const val ALIAS = "app.norishell.vault-auto-unlock"
    private const val PROVIDER = "AndroidKeyStore"
    private const val TRANSFORMATION = "AES/GCM/NoPadding"
    private const val IV_LENGTH = 12
    private const val TAG_BITS = 128

    class KeystoreException(val code: String, cause: Throwable? = null) : Exception(code, cause)

    private fun keyStore(): KeyStore = try {
        KeyStore.getInstance(PROVIDER).apply { load(null) }
    } catch (error: Exception) {
        throw KeystoreException("keystore.unavailable", error)
    }

    private fun existingKey(store: KeyStore): SecretKey? = try {
        store.getKey(ALIAS, null) as? SecretKey
    } catch (error: Exception) {
        throw KeystoreException("keystore.failed", error)
    }

    private fun generate(strongBox: Boolean): SecretKey {
        val builder = KeyGenParameterSpec.Builder(
            ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
        )
            .setKeySize(256)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setRandomizedEncryptionRequired(true)
            .setUserAuthenticationRequired(false)
        if (strongBox && Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            builder.setIsStrongBoxBacked(true)
        }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, PROVIDER)
        generator.init(builder.build())
        return generator.generateKey()
    }

    private fun createKey(): SecretKey = try {
        try {
            generate(strongBox = true)
        } catch (error: StrongBoxUnavailableException) {
            generate(strongBox = false)
        }
    } catch (error: KeystoreException) {
        throw error
    } catch (error: Exception) {
        throw KeystoreException("keystore.failed", error)
    }

    @Synchronized
    fun wrap(plaintext: ByteArray): ByteArray {
        val store = keyStore()
        var key = existingKey(store) ?: createKey()
        val cipher = Cipher.getInstance(TRANSFORMATION)
        try {
            cipher.init(Cipher.ENCRYPT_MODE, key)
        } catch (error: KeyPermanentlyInvalidatedException) {
            deleteEntry(store)
            key = createKey()
            cipher.init(Cipher.ENCRYPT_MODE, key)
        } catch (error: Exception) {
            throw KeystoreException("keystore.failed", error)
        }
        return try {
            val iv = cipher.iv
            if (iv.size != IV_LENGTH) throw KeystoreException("keystore.failed")
            iv + cipher.doFinal(plaintext)
        } catch (error: KeystoreException) {
            throw error
        } catch (error: Exception) {
            throw KeystoreException("keystore.failed", error)
        }
    }

    @Synchronized
    fun unwrap(blob: ByteArray): ByteArray {
        if (blob.size <= IV_LENGTH + TAG_BITS / 8) throw KeystoreException("keystore.failed")
        val store = keyStore()
        val key = existingKey(store) ?: throw KeystoreException("keystore.key_missing")
        val cipher = Cipher.getInstance(TRANSFORMATION)
        try {
            cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(TAG_BITS, blob, 0, IV_LENGTH))
            return cipher.doFinal(blob, IV_LENGTH, blob.size - IV_LENGTH)
        } catch (error: KeyPermanentlyInvalidatedException) {
            deleteEntry(store)
            throw KeystoreException("keystore.key_missing", error)
        } catch (error: AEADBadTagException) {
            throw KeystoreException("keystore.key_missing", error)
        } catch (error: Exception) {
            throw KeystoreException("keystore.failed", error)
        }
    }

    @Synchronized
    fun delete() {
        deleteEntry(keyStore())
    }

    private fun deleteEntry(store: KeyStore) {
        try {
            if (store.containsAlias(ALIAS)) store.deleteEntry(ALIAS)
        } catch (error: Exception) {
            throw KeystoreException("keystore.failed", error)
        }
    }
}
