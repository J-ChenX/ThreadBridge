package dev.threadbridge

import android.graphics.Bitmap
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import kotlinx.coroutines.CancellationException

object ConversationAttachments {
 private val marker=Regex("(?m)^!\\[图片]\\(threadbridge-image:([a-f0-9]{64})\\)\\s*$")
 fun images(text:String)=marker.findAll(text).map{it.groupValues[1]}.toList().distinct()
 fun caption(text:String)=marker.replace(text,"").trim()
}

@Composable fun UserMessageWithImages(repo:Repository,message:MessageRow,text:String,bubble:@Composable (String)->Unit) {
 val images=remember(text){ConversationAttachments.images(text)}
 val caption=remember(text){ConversationAttachments.caption(text)}
 Column(horizontalAlignment=Alignment.End,verticalArrangement=Arrangement.spacedBy(8.dp)) {
  for(id in images) key(id) { UserImage(repo,message,id) }
  if(caption.isNotEmpty())bubble(caption)
 }
}

@Composable private fun UserImage(repo:Repository,message:MessageRow,id:String) {
 var bitmap by remember(message.threadId,id){mutableStateOf<Bitmap?>(null)}
 var failed by remember{mutableStateOf(false)}
 var attempt by remember{mutableIntStateOf(0)}
 var expanded by remember{mutableStateOf(false)}
 LaunchedEffect(message.threadId,id,attempt) {
  failed=false
  try {bitmap=repo.userImage(message,id)}catch(e:CancellationException){throw e}catch(_:Exception){failed=true}
 }
 val loaded=bitmap
 if(loaded==null)Surface(modifier=Modifier.width(156.dp).height(100.dp),shape=MaterialTheme.shapes.medium) {
  Box(contentAlignment=Alignment.Center) {
   if(failed)TextButton(onClick={attempt++}){Text("重试加载图片")}
   else CircularProgressIndicator(Modifier.size(20.dp),strokeWidth=2.dp)
  }
 }else {
  Image(loaded.asImageBitmap(),"图片附件",modifier=Modifier.widthIn(max=220.dp).heightIn(max=180.dp).clickable{expanded=true},contentScale=ContentScale.Fit)
  if(expanded)Dialog(onDismissRequest={expanded=false}) {
   Surface(shape=MaterialTheme.shapes.medium) {
    Column(horizontalAlignment=Alignment.CenterHorizontally) {
     Image(loaded.asImageBitmap(),"图片预览",modifier=Modifier.fillMaxWidth().heightIn(max=600.dp),contentScale=ContentScale.Fit)
     TextButton(onClick={expanded=false}){Text("关闭")}
    }
   }
  }
 }
}
