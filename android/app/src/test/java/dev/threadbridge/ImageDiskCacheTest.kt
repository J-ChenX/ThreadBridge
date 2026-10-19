package dev.threadbridge

import org.junit.Assert.*
import org.junit.Test
import java.io.File
import java.nio.file.Files
import java.security.MessageDigest

class ImageDiskCacheTest {
 private fun digest(bytes:ByteArray)=MessageDigest.getInstance("SHA-256").digest(bytes).joinToString(""){"%02x".format(it)}
 @Test fun originalSurvivesRecreationAndCorruptionIsRemoved(){
  val dir=Files.createTempDirectory("image-cache").toFile()
  try{val file=File(dir,"photo");val data="original photo".toByteArray();ImageDiskCache(dir).write(file,digest(data),data)
   assertArrayEquals(data,ImageDiskCache(dir).read(file,digest(data)))
   file.writeText("truncated");assertNull(ImageDiskCache(dir).read(file,digest(data)));assertFalse(file.exists())
   ImageDiskCache(dir).write(file,digest(data),data);assertArrayEquals(data,ImageDiskCache(dir).read(file,digest(data)))
   assertFalse(dir.listFiles()!!.any{it.extension=="part"})
  }finally{dir.deleteRecursively()}
 }
 @Test fun recentlyViewedOriginalWinsEvictionAndHashMismatchCannotPoisonCache(){
  val dir=Files.createTempDirectory("image-cache").toFile()
  try{val cache=ImageDiskCache(dir,8);val data="1234".toByteArray();val hash=digest(data)
   val a=File(dir,"a");val b=File(dir,"b");val c=File(dir,"c")
   cache.write(a,hash,data);cache.write(b,hash,data);a.setLastModified(1);b.setLastModified(2)
   cache.read(a,hash);cache.write(c,hash,data);assertTrue(a.exists());assertFalse(b.exists());assertTrue(c.exists())
   try{cache.write(a,hash,"bad".toByteArray());fail("Invalid image accepted")}catch(_:IllegalStateException){}
   assertArrayEquals(data,cache.read(a,hash))
  }finally{dir.deleteRecursively()}
 }
}
