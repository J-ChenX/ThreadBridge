package dev.threadbridge

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.graphics.Bitmap
import android.os.Handler
import android.os.Looper
import android.view.PixelCopy
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import kotlin.coroutines.resume

/** Capture only our activity window; a small CPU blur also works before Android 12. */
internal suspend fun imageBackdrop(context:Context):Bitmap? {
 var owner=context
 while(owner is ContextWrapper&&owner !is Activity)owner=owner.baseContext
 val window=(owner as? Activity)?.window?:return null
 val view=window.decorView
 if(view.width<=0||view.height<=0)return null
 val bitmap=Bitmap.createBitmap(maxOf(1,view.width/12),maxOf(1,view.height/12),Bitmap.Config.ARGB_8888)
 val captured=suspendCancellableCoroutine<Boolean>{continuation->
  try{PixelCopy.request(window,bitmap,{result->if(continuation.isActive)continuation.resume(result==PixelCopy.SUCCESS)},Handler(Looper.getMainLooper()))}
  catch(_:IllegalArgumentException){continuation.resume(false)}
 }
 if(!captured)return null
 return withContext(Dispatchers.Default){
  val w=bitmap.width;val h=bitmap.height;var source=IntArray(w*h);var target=IntArray(w*h)
  bitmap.getPixels(source,0,w,0,0,w,h)
  repeat(3){
   for(y in 0 until h)for(x in 0 until w){
    var red=0;var green=0;var blue=0;var count=0
    for(dy in -2..2)for(dx in -2..2){val pixel=source[(y+dy).coerceIn(0,h-1)*w+(x+dx).coerceIn(0,w-1)];red+=pixel shr 16 and 255;green+=pixel shr 8 and 255;blue+=pixel and 255;count++}
    target[y*w+x]=(255 shl 24) or ((red/count) shl 16) or ((green/count) shl 8) or (blue/count)
   }
   val old=source;source=target;target=old
  }
  bitmap.setPixels(source,0,w,0,0,w,h);bitmap
 }
}
