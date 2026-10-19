package dev.threadbridge

import android.content.ContentValues
import android.content.Context
import android.graphics.BitmapFactory
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

internal data class OriginalImage(val bytes:ByteArray,val mime:String,val extension:String)
internal fun originalImage(bytes:ByteArray):OriginalImage {
 val options=BitmapFactory.Options().apply{inJustDecodeBounds=true}
 BitmapFactory.decodeByteArray(bytes,0,bytes.size,options)
 val mime=options.outMimeType?:error("无法读取图片格式")
 val extension=when(mime){"image/jpeg"->"jpg";"image/png"->"png";"image/webp"->"webp";"image/gif"->"gif";else->error("暂不支持此图片格式")}
 return OriginalImage(bytes,mime,extension)
}
internal suspend fun writeImage(context:Context,uri:Uri,image:OriginalImage)=withContext(Dispatchers.IO){
 (context.contentResolver.openOutputStream(uri)?:error("无法写入图片")).use{it.write(image.bytes)}
}
internal suspend fun downloadImage(context:Context,image:OriginalImage,name:String)=withContext(Dispatchers.IO){
 check(Build.VERSION.SDK_INT>=29)
 val resolver=context.contentResolver
 val values=ContentValues().apply{put(MediaStore.Images.Media.DISPLAY_NAME,name);put(MediaStore.Images.Media.MIME_TYPE,image.mime);put(MediaStore.Images.Media.RELATIVE_PATH,"Pictures/ThreadBridge");put(MediaStore.Images.Media.IS_PENDING,1)}
 val uri=resolver.insert(MediaStore.Images.Media.EXTERNAL_CONTENT_URI,values)?:error("无法创建图片文件")
 var saved=false
 try{writeImage(context,uri,image);check(resolver.update(uri,ContentValues().apply{put(MediaStore.Images.Media.IS_PENDING,0)},null,null)>0);saved=true}
 finally{if(!saved)resolver.delete(uri,null,null)}
}
