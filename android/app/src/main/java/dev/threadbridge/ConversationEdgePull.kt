package dev.threadbridge

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.nestedscroll.*
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Velocity
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** Positive unconsumed motion is beyond the oldest/top edge of a reverse list;
 * negative motion is beyond its newest/bottom edge. Never steal normal scrolling.
 */
internal fun conversationPullAllowed(delta:Float,atNewest:Boolean,atOldest:Boolean,refreshing:Boolean,loadingOlder:Boolean,canLoadOlder:Boolean):Boolean =
 if(delta<0f)atNewest&&!refreshing else delta>0f&&atOldest&&!loadingOlder&&canLoadOlder

internal class ConversationEdgePull(val threshold:Float):NestedScrollConnection {
 var distance by mutableFloatStateOf(0f)
 var allowed:(Float)->Boolean={false}
 var refresh:()->Unit={}
 var older:()->Unit={}
 override fun onPreScroll(available:Offset,source:NestedScrollSource):Offset {
  if(source!=NestedScrollSource.UserInput||distance==0f||distance*available.y>=0f)return Offset.Zero
  val previous=distance
  distance=if(previous>0f)(previous+available.y).coerceAtLeast(0f) else (previous+available.y).coerceAtMost(0f)
  return Offset(0f,distance-previous)
 }
 override fun onPostScroll(consumed:Offset,available:Offset,source:NestedScrollSource):Offset {
  if(source!=NestedScrollSource.UserInput||!allowed(available.y))return Offset.Zero
  distance=(distance+available.y).coerceIn(-threshold*1.6f,threshold*1.6f)
  return Offset(0f,available.y)
 }
 override suspend fun onPreFling(available:Velocity):Velocity {
  val pulled=distance
  distance=0f
  if(allowed(pulled)) {
   if(pulled<=-threshold)refresh() else if(pulled>=threshold)older()
  }
  return if(pulled==0f)Velocity.Zero else Velocity(0f,available.y)
 }
}

@Composable internal fun rememberConversationEdgePull(list:LazyListState,refreshing:Boolean,loadingOlder:Boolean,canLoadOlder:Boolean,refresh:()->Unit,older:()->Unit):ConversationEdgePull {
 val threshold=with(LocalDensity.current){96.dp.toPx()}
 val pull=remember(list,threshold){ConversationEdgePull(threshold)}
 SideEffect {
  pull.allowed={delta->conversationPullAllowed(delta,!list.canScrollBackward,!list.canScrollForward,refreshing,loadingOlder,canLoadOlder)}
  pull.refresh=refresh;pull.older=older
 }
 return pull
}
internal fun Modifier.conversationEdgePull(pull:ConversationEdgePull)=nestedScroll(pull)

@Composable internal fun EdgePullIndicator(progress:Float,loading:Boolean,label:String,modifier:Modifier) {
 Surface(modifier=modifier,shape=CircleShape,color=MaterialTheme.colorScheme.surface,shadowElevation=1.dp) {
  Row(Modifier.padding(horizontal=12.dp,vertical=8.dp),verticalAlignment=Alignment.CenterVertically) {
   if(loading)CircularProgressIndicator(Modifier.size(16.dp),strokeWidth=2.dp)
   else Text((if(progress>=1f)"松开以"else"继续拉动以")+label,fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)
  }
 }
}
