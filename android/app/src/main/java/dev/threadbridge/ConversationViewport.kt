package dev.threadbridge

import androidx.compose.animation.*
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalWindowInfo
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest

@Composable internal fun ConversationIconButton(floating:Boolean,onClick:()->Unit,content:@Composable ()->Unit) {
 if(floating)Surface(shape=CircleShape,color=MaterialTheme.colorScheme.surface,shadowElevation=1.dp) {
  BridgeIconButton(onClick=onClick,content=content)
 }else BridgeIconButton(onClick=onClick,content=content)
}

@Composable internal fun ConversationHeader(navigation:@Composable ()->Unit) {
 Row(Modifier.fillMaxWidth().windowInsetsPadding(WindowInsets.statusBars.union(WindowInsets.displayCutout.only(WindowInsetsSides.Top))).padding(horizontal=12.dp,vertical=4.dp),verticalAlignment=Alignment.CenterVertically){navigation()}
}

/** Keep the scroll viewport behind the chrome. The keyboard changes its bounds.
 * Insets animate in the layout phase; composer motion runs concurrently.
 * Measured padding keeps the first/last messages reachable, including large fonts.
 */
@Composable internal fun ConversationViewport(
 list:LazyListState,header:@Composable ()->Unit,atLatest:Boolean,newest:()->Unit,
 refreshing:Boolean,refresh:()->Unit,loadingOlder:Boolean,canLoadOlder:Boolean,loadOlder:()->Unit,
 composer:@Composable ()->Unit,composerMotion:()->Float,loading:Boolean,content:LazyListScope.()->Unit
) {
 val density=LocalDensity.current
 val windowWidth=with(density){LocalWindowInfo.current.containerSize.width.toDp()}
 val contentGutter=maxOf(16.dp,(windowWidth-720.dp)/2)
 var headerHeight by remember { mutableStateOf(0.dp) }
 var composerHeight by remember { mutableStateOf(0.dp) }
 val background=MaterialTheme.colorScheme.background
 val topInsets=WindowInsets.statusBars.union(WindowInsets.displayCutout.only(WindowInsetsSides.Top))
 val expandedBackground=MaterialTheme.colorScheme.surfaceContainerLow
 var jumpVisible by remember { mutableStateOf(false) }
 LaunchedEffect(list) {
  snapshotFlow { list.isScrollInProgress }.collectLatest { scrolling->
   if(scrolling)jumpVisible=true else {delay(3000);jumpVisible=false}
  }
 }
 val edgePull=rememberConversationEdgePull(list,refreshing,loadingOlder,canLoadOlder,refresh,loadOlder)
 Box(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal)).imePadding()) {
  LazyColumn(Modifier.fillMaxSize().conversationEdgePull(edgePull),state=list,reverseLayout=true,
   contentPadding=PaddingValues(start=contentGutter,end=contentGutter,top=headerHeight+8.dp,bottom=composerHeight+8.dp),
   verticalArrangement=Arrangement.spacedBy(16.dp),content=content)
  if(loading)CircularProgressIndicator(Modifier.size(22.dp).align(Alignment.Center),strokeWidth=2.dp)
  // Drawing-only scrims do not intercept scrolling or text selection beneath them.
  Column(Modifier.align(Alignment.TopCenter).fillMaxWidth().onSizeChanged{headerHeight=with(density){it.height.toDp()}}.drawWithCache {
   // Clamp the gradient to opaque until below the actual status/cutout area.
   val start=minOf(topInsets.getTop(this).toFloat(),size.height)
   val fade=Brush.verticalGradient(0f to background,0.28f to background.copy(alpha=0.9f),0.6f to background.copy(alpha=0.55f),1f to background.copy(alpha=0f),startY=start,endY=maxOf(start+1f,size.height))
   onDrawBehind { drawRect(fade) }
  }) {
   header();Spacer(Modifier.height(12.dp))
  }
  Column(Modifier.align(Alignment.BottomCenter).widthIn(max=720.dp).fillMaxWidth().onSizeChanged{composerHeight=with(density){it.height.toDp()}}.drawWithCache {
   val tint=lerp(background,expandedBackground,0.45f+0.55f*composerMotion().coerceIn(0f,1f))
   val fade=Brush.verticalGradient(0f to tint.copy(alpha=0f),0.25f to tint.copy(alpha=0.12f),0.55f to tint.copy(alpha=0.35f),0.85f to tint.copy(alpha=0.9f),1f to tint)
   onDrawBehind { drawRect(fade) }
  }.navigationBarsPadding()) {
   Spacer(Modifier.height(16.dp));composer()
  }
  if(edgePull.distance>0f||loadingOlder)EdgePullIndicator(edgePull.distance/edgePull.threshold,loadingOlder,"加载更早消息",Modifier.align(Alignment.TopCenter).padding(top=headerHeight+4.dp))
  if(edgePull.distance<0f||(refreshing&&atLatest))EdgePullIndicator(-edgePull.distance/edgePull.threshold,refreshing,"同步对话",Modifier.align(Alignment.BottomCenter).padding(bottom=composerHeight+8.dp))
  AnimatedVisibility(visible=!atLatest&&jumpVisible&&edgePull.distance==0f,enter=fadeIn(tween(BridgeMotion.Fade))+scaleIn(tween(BridgeMotion.Icon),initialScale=0.92f),exit=fadeOut(tween(BridgeMotion.Press))+scaleOut(tween(BridgeMotion.Release),targetScale=0.94f),modifier=Modifier.align(Alignment.BottomCenter).padding(bottom=composerHeight+8.dp)){
   Surface(shape=CircleShape,color=MaterialTheme.colorScheme.surface,shadowElevation=1.dp){BridgeIconButton(onClick={jumpVisible=false;newest()}){Icon(BridgeIcons.Down,"回到最新消息",Modifier.size(20.dp))}}
  }
 }
}
