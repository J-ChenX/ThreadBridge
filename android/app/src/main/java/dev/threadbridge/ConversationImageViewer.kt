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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.window.DialogWindowProvider
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.core.view.WindowCompat
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
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import android.os.Build
import android.widget.Toast

@Composable internal fun ConversationImageViewer(repo:Repository,message:MessageRow,id:String,thumbnail:Bitmap,close:()->Unit) {
 var bitmap by remember(id){mutableStateOf(thumbnail)}

 var failed by remember(id){mutableStateOf(false)}
 var attempt by remember(id){mutableIntStateOf(0)}
 LaunchedEffect(id,attempt){failed=false;try{bitmap=repo.userImage(message,id,1600)}catch(e:CancellationException){throw e}catch(_:Exception){failed=true}}
 var transform by remember(id){mutableStateOf(ImageTransform())}
 var viewport by remember{mutableStateOf(IntSize.Zero)}
 val context=LocalContext.current
 var backdrop by remember{mutableStateOf<Bitmap?>(null)}
 LaunchedEffect(Unit){backdrop=imageBackdrop(context)}
 var downloading by remember{mutableStateOf(false)}
 var export by remember{mutableStateOf<OriginalImage?>(null)}
 val scope=rememberCoroutineScope();var motion by remember{mutableStateOf<Job?>(null)}
 fun constrain(value:ImageTransform)=constrainImage(value,viewport.width.toFloat(),viewport.height.toFloat(),bitmap.width.toFloat(),bitmap.height.toFloat())
 fun animate(target:ImageTransform){
  motion?.cancel();val start=transform
  motion=scope.launch{Animatable(0f).animateTo(1f,tween(BridgeMotion.Reveal)){val f=value;transform=constrain(ImageTransform(start.scale+(target.scale-start.scale)*f,start.x+(target.x-start.x)*f,start.y+(target.y-start.y)*f))}}
 }
 val document=rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("image/*")){uri->
  val original=export;export=null
  if(uri!=null&&original!=null)scope.launch{downloading=true;try{writeImage(context,uri,original);Toast.makeText(context,"图片已保存",Toast.LENGTH_SHORT).show()}catch(e:CancellationException){throw e}catch(_:Exception){Toast.makeText(context,"图片保存失败，请重试",Toast.LENGTH_LONG).show()}finally{downloading=false}}
 }
 fun download(){if(downloading||export!=null)return;downloading=true;scope.launch{
  try{
   val original=originalImage(repo.originalImage(message,id));val name="ThreadBridge-${id.take(12)}-${System.currentTimeMillis()}.${original.extension}"
   if(Build.VERSION.SDK_INT>=29){downloadImage(context,original,name);Toast.makeText(context,"已保存到相册 Pictures/ThreadBridge",Toast.LENGTH_LONG).show()}
   else{export=original;document.launch(name)}
  }catch(e:CancellationException){throw e}catch(_:Exception){export=null;Toast.makeText(context,"图片保存失败，请检查连接或存储空间后重试",Toast.LENGTH_LONG).show()}finally{downloading=false}
 }}
 Dialog(onDismissRequest=close,properties=DialogProperties(usePlatformDefaultWidth=false,decorFitsSystemWindows=false)) {
  val dialogWindow=(LocalView.current.parent as? DialogWindowProvider)?.window
  val dark=isSystemInDarkTheme()
  SideEffect{dialogWindow?.let{window->WindowCompat.getInsetsController(window,window.decorView).apply{isAppearanceLightStatusBars=!dark;isAppearanceLightNavigationBars=!dark}}}
  Box(Modifier.fillMaxSize()) {
   backdrop?.let{Image(it.asImageBitmap(),null,Modifier.fillMaxSize(),contentScale=ContentScale.FillBounds)}
   Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.scrim.copy(alpha=0.42f)))
   Box(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing)) {
    Box(Modifier.fillMaxSize().onSizeChanged{viewport=it;motion?.cancel();transform=constrain(transform)}.clipToBounds()
     .semantics{contentDescription="图片预览";stateDescription="缩放 ${(transform.scale*100).roundToInt()}%";onClick("关闭图片预览"){close();true}}
     .pointerInput(id){detectTapGestures(onTap={point->
      if(!imageContains(transform,point.x,point.y,viewport.width.toFloat(),viewport.height.toFloat(),bitmap.width.toFloat(),bitmap.height.toFloat()))close()
     },onDoubleTap={point->
      if(!imageContains(transform,point.x,point.y,viewport.width.toFloat(),viewport.height.toFloat(),bitmap.width.toFloat(),bitmap.height.toFloat()))close()
      else{val target=if(transform.scale>1.05f)ImageTransform()else imageGesture(transform,2.5f,0f,0f,point.x,point.y,viewport.width.toFloat(),viewport.height.toFloat(),bitmap.width.toFloat(),bitmap.height.toFloat());animate(target)}
     })}
     .pointerInput(id){detectTransformGestures{centroid,pan,zoom,_->
      motion?.cancel();transform=imageGesture(transform,zoom,pan.x,pan.y,centroid.x,centroid.y,viewport.width.toFloat(),viewport.height.toFloat(),bitmap.width.toFloat(),bitmap.height.toFloat())
     }},contentAlignment=Alignment.Center) {
     Image(bitmap.asImageBitmap(),null,Modifier.fillMaxSize().graphicsLayer{scaleX=transform.scale;scaleY=transform.scale;translationX=transform.x;translationY=transform.y},contentScale=ContentScale.Fit)
    }
    if(failed)Surface(shape=CircleShape,color=MaterialTheme.colorScheme.surface,modifier=Modifier.align(Alignment.BottomStart).padding(16.dp)){
     TextButton(onClick={attempt++}){Text("重试加载清晰图片",fontSize=12.sp)}
    }
    Surface(shape=CircleShape,color=MaterialTheme.colorScheme.surface,shadowElevation=2.dp,modifier=Modifier.align(Alignment.BottomEnd).padding(16.dp)){
     BridgeIconButton(onClick=::download,enabled=!downloading&&export==null){
      if(downloading)CircularProgressIndicator(Modifier.size(20.dp),strokeWidth=2.dp)
      else Icon(BridgeIcons.Download,"下载图片",Modifier.size(22.dp))
     }
    }
   }
  }
 }
}
