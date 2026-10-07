package dev.threadbridge

import android.graphics.Bitmap
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

@Composable internal fun ConversationImageViewer(repo:Repository,message:MessageRow,id:String,thumbnail:Bitmap,close:()->Unit) {
 var bitmap by remember(id){mutableStateOf(thumbnail)}
 var loading by remember(id){mutableStateOf(true)}
 var failed by remember(id){mutableStateOf(false)}
 var attempt by remember(id){mutableIntStateOf(0)}
 LaunchedEffect(id,attempt){loading=true;failed=false;try{bitmap=repo.userImage(message,id,1600)}catch(e:CancellationException){throw e}catch(_:Exception){failed=true}finally{loading=false}}
 var transform by remember(id){mutableStateOf(ImageTransform())}
 var viewport by remember{mutableStateOf(IntSize.Zero)}
 val density=LocalDensity.current
 var footerHeight by remember{mutableStateOf(88.dp)}
 val scope=rememberCoroutineScope();var motion by remember{mutableStateOf<Job?>(null)}
 fun constrain(value:ImageTransform)=constrainImage(value,viewport.width.toFloat(),viewport.height.toFloat(),bitmap.width.toFloat(),bitmap.height.toFloat())
 fun animate(target:ImageTransform){
  motion?.cancel();val start=transform
  motion=scope.launch{Animatable(0f).animateTo(1f,tween(BridgeMotion.Reveal)){val f=value;transform=constrain(ImageTransform(start.scale+(target.scale-start.scale)*f,start.x+(target.x-start.x)*f,start.y+(target.y-start.y)*f))}}
 }
 Dialog(onDismissRequest=close,properties=DialogProperties(usePlatformDefaultWidth=false,decorFitsSystemWindows=false)) {
  Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).windowInsetsPadding(WindowInsets.safeDrawing)) {
   Box(Modifier.fillMaxSize().padding(top=64.dp,bottom=footerHeight+8.dp).onSizeChanged{viewport=it;motion?.cancel();transform=constrain(transform)}.clipToBounds()
    .semantics{contentDescription="图片预览";stateDescription="缩放 ${(transform.scale*100).roundToInt()}%"}
    .pointerInput(id){detectTapGestures(onDoubleTap={point->
     val target=if(transform.scale>1.05f)ImageTransform()else imageGesture(transform,2.5f,0f,0f,point.x,point.y,viewport.width.toFloat(),viewport.height.toFloat(),bitmap.width.toFloat(),bitmap.height.toFloat())
     animate(target)
    })}
    .pointerInput(id){detectTransformGestures{centroid,pan,zoom,_->
     motion?.cancel();transform=imageGesture(transform,zoom,pan.x,pan.y,centroid.x,centroid.y,viewport.width.toFloat(),viewport.height.toFloat(),bitmap.width.toFloat(),bitmap.height.toFloat())
    }},contentAlignment=Alignment.Center) {
    Image(bitmap.asImageBitmap(),null,Modifier.fillMaxSize().graphicsLayer{scaleX=transform.scale;scaleY=transform.scale;translationX=transform.x;translationY=transform.y},contentScale=ContentScale.Fit)
   }
   Row(Modifier.fillMaxWidth().align(Alignment.TopCenter).padding(horizontal=12.dp,vertical=8.dp),verticalAlignment=Alignment.CenterVertically){
    BridgeIconButton(onClick=close){Icon(BridgeIcons.Close,"关闭图片预览",Modifier.size(20.dp))}
    Text("图片",fontSize=15.sp,modifier=Modifier.weight(1f).padding(start=8.dp))
    if(loading)CircularProgressIndicator(Modifier.padding(14.dp).size(20.dp),strokeWidth=2.dp)
   }
   Column(Modifier.align(Alignment.BottomCenter).onSizeChanged{footerHeight=with(density){it.height.toDp()}}.padding(bottom=12.dp),horizontalAlignment=Alignment.CenterHorizontally,verticalArrangement=Arrangement.spacedBy(8.dp)){
    Surface(shape=CircleShape,color=MaterialTheme.colorScheme.surfaceContainerLow){
     Row(Modifier.padding(horizontal=8.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(8.dp)){
      BridgeIconButton(onClick={animate(ImageTransform(maxOf(1f,transform.scale/1.5f),transform.x,transform.y))},enabled=transform.scale>1.01f){Icon(BridgeIcons.Minus,"缩小图片",Modifier.size(20.dp))}
      Text("${(transform.scale*100).roundToInt()}%",fontSize=13.sp,modifier=Modifier.widthIn(min=44.dp))
      BridgeIconButton(onClick={animate(ImageTransform(minOf(5f,transform.scale*1.5f),transform.x,transform.y))},enabled=transform.scale<4.99f){Icon(BridgeIcons.Add,"放大图片",Modifier.size(20.dp))}
      TextButton(onClick={animate(ImageTransform())},enabled=transform.scale>1.01f){Text("复位",fontSize=13.sp)}
     }
    }
    if(failed)TextButton(onClick={attempt++}){Text("重试加载清晰图片",fontSize=12.sp)}
    else Text("双指缩放 · 双击放大",fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)
   }
  }
 }
}
