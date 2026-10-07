package dev.threadbridge

import kotlin.math.min

internal data class ImageTransform(val scale:Float=1f,val x:Float=0f,val y:Float=0f)
internal fun constrainImage(value:ImageTransform,width:Float,height:Float,imageWidth:Float,imageHeight:Float):ImageTransform {
 if(width<=0||height<=0||imageWidth<=0||imageHeight<=0)return ImageTransform()
 val scale=value.scale.coerceIn(1f,5f)
 val fit=min(width/imageWidth,height/imageHeight)
 val dx=maxOf(0f,(imageWidth*fit*scale-width)/2)
 val dy=maxOf(0f,(imageHeight*fit*scale-height)/2)
 return ImageTransform(scale,if(dx==0f)0f else value.x.coerceIn(-dx,dx),if(dy==0f)0f else value.y.coerceIn(-dy,dy))
}
internal fun imageGesture(value:ImageTransform,zoom:Float,panX:Float,panY:Float,focusX:Float,focusY:Float,width:Float,height:Float,imageWidth:Float,imageHeight:Float):ImageTransform {
 val scale=(value.scale*zoom).coerceIn(1f,5f);val ratio=scale/value.scale
 return constrainImage(ImageTransform(scale,value.x*ratio+(focusX-width/2)*(1-ratio)+panX,value.y*ratio+(focusY-height/2)*(1-ratio)+panY),width,height,imageWidth,imageHeight)
}
