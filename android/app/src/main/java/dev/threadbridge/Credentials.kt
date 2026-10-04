package dev.threadbridge

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

class Credentials(context:Context) {
 private val prefs=context.getSharedPreferences("connection",Context.MODE_PRIVATE)
 val server get()=prefs.getString("server","")!!
 val lan get()=prefs.getBoolean("lan",false)
 private fun key():SecretKey {
  val store=KeyStore.getInstance("AndroidKeyStore").apply{load(null)}
  (store.getKey("threadbridge",null) as? SecretKey)?.let{return it}
  return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES,"AndroidKeyStore").apply{init(KeyGenParameterSpec.Builder("threadbridge",KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT).setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build())}.generateKey()
 }
 fun token():String { val raw=prefs.getString("token",null)?:return "";return try {val bytes=Base64.decode(raw,Base64.NO_WRAP);val c=Cipher.getInstance("AES/GCM/NoPadding");c.init(Cipher.DECRYPT_MODE,key(),GCMParameterSpec(128,bytes.copyOfRange(0,12)));String(c.doFinal(bytes.copyOfRange(12,bytes.size)),Charsets.UTF_8)}catch(_:Exception){""} }
 fun save(server:String,token:String,lan:Boolean){val c=Cipher.getInstance("AES/GCM/NoPadding");c.init(Cipher.ENCRYPT_MODE,key());val value=Base64.encodeToString(c.iv+c.doFinal(token.toByteArray()),Base64.NO_WRAP);check(prefs.edit().putString("server",server).putString("token",value).putBoolean("lan",lan).commit())}
 fun clear(){prefs.edit().clear().commit()}
}
