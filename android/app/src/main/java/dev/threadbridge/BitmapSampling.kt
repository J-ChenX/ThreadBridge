package dev.threadbridge

internal fun bitmapSample(width:Int,height:Int,limit:Int):Int {
 require(width>0&&height>0&&limit>0)
 var sample=1
 while(maxOf(width,height)/sample>limit&&sample<=Int.MAX_VALUE/2)sample*=2
 return sample
}
