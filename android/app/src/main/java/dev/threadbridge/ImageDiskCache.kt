package dev.threadbridge

import java.io.File
import java.security.MessageDigest

/** Called under the image mutex. Only verified complete originals enter the cache. */
internal class ImageDiskCache(private val directory:File,private val budget:Long=128L*1024*1024) {
 private fun valid(bytes:ByteArray,digest:String)=MessageDigest.getInstance("SHA-256").digest(bytes).joinToString(""){"%02x".format(it)}==digest
 fun read(file:File,digest:String):ByteArray? {
  if(!file.isFile)return null
  val bytes=try{if(file.length()>20L*1024*1024)null else file.readBytes()}catch(_:java.io.IOException){null}
  if(bytes==null||!valid(bytes,digest)){file.delete();return null}
  file.setLastModified(System.currentTimeMillis());return bytes
 }
 fun write(file:File,digest:String,bytes:ByteArray) {
  check(valid(bytes,digest)){"图片校验失败"}
  require(bytes.size<=budget)
  directory.mkdirs()
  val stage=File.createTempFile("image-",".part",directory)
  try {
   stage.outputStream().use{it.write(bytes);it.fd.sync()}
   var used=directory.listFiles()?.filter{it!=stage&&it!=file}?.sumOf{it.length()}?:0
   for(old in directory.listFiles()?.filter{it!=stage&&it!=file}?.sortedBy{it.lastModified()}?:emptyList()){
    if(used+bytes.size<=budget)break
    val length=old.length();if(old.delete())used-=length
   }
   check(stage.renameTo(file)){"无法缓存图片"}
  }finally{stage.delete()}
 }
}
