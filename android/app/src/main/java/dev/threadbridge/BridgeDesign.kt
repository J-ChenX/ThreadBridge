package dev.threadbridge

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.*
import androidx.compose.foundation.*
import androidx.compose.foundation.interaction.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.*
import androidx.compose.ui.graphics.drawscope.ContentDrawScope
import androidx.compose.ui.graphics.vector.*
import androidx.compose.ui.node.DrawModifierNode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.launch

internal object BridgeMotion {
 const val Press=90
 const val Release=140
 const val Icon=180
 const val Reveal=220
 const val Fade=120
}
internal object BridgeShapes {
 val Row=RoundedCornerShape(16.dp)
 val Panel=RoundedCornerShape(28.dp)
 val Field=RoundedCornerShape(16.dp)
}

/** A quiet, bounded state layer. Animation runs in drawing, not chat recomposition. */
private data class SoftIndication(val color:Color):IndicationNodeFactory {
 override fun create(interactionSource:InteractionSource):Modifier.Node=SoftIndicationNode(color,interactionSource)
}
private class SoftIndicationNode(val color:Color,val source:InteractionSource):Modifier.Node(),DrawModifierNode {
 private val strength=Animatable(0f)
 private var focused by mutableStateOf(false)
 override fun onAttach(){coroutineScope.launch{
  val held=mutableSetOf<Interaction>()
  source.interactions.collectLatest{interaction->
   when(interaction){
    is PressInteraction.Press,is HoverInteraction.Enter->held.add(interaction)
    is FocusInteraction.Focus->{focused=true;held.add(interaction)}
    is PressInteraction.Release->held.remove(interaction.press)
    is PressInteraction.Cancel->held.remove(interaction.press)
    is FocusInteraction.Unfocus->{focused=false;held.remove(interaction.focus)}
    is HoverInteraction.Exit->held.remove(interaction.enter)
   }
   val active=held.isNotEmpty()
   strength.animateTo(if(active)0.075f else 0f,tween(if(active)BridgeMotion.Press else BridgeMotion.Release))
  }
 }}
 override fun ContentDrawScope.draw(){
  drawContent();drawRect(color.copy(alpha=strength.value))
  if(focused){val inset=2.dp.toPx();drawRoundRect(color.copy(alpha=0.65f),topLeft=androidx.compose.ui.geometry.Offset(inset,inset),size=androidx.compose.ui.geometry.Size((size.width-2*inset).coerceAtLeast(0f),(size.height-2*inset).coerceAtLeast(0f)),cornerRadius=androidx.compose.ui.geometry.CornerRadius(size.height/2),style=androidx.compose.ui.graphics.drawscope.Stroke(1.5.dp.toPx()))}
 }
}
@Composable internal fun BridgeInteractionTheme(content:@Composable ()->Unit){
 CompositionLocalProvider(LocalIndication provides rememberBridgeIndication(),content=content)
}
@Composable internal fun rememberBridgeIndication():IndicationNodeFactory {
 val color=MaterialTheme.colorScheme.onSurface
 return remember(color){SoftIndication(color)}
}
internal fun Modifier.softClickable(enabled:Boolean=true,shape:Shape=BridgeShapes.Row,source:MutableInteractionSource?=null,onClick:()->Unit)=composed {
 val interactions=source?:remember{MutableInteractionSource()}
 clip(shape).clickable(enabled=enabled,interactionSource=interactions,indication=rememberBridgeIndication(),role=Role.Button,onClick=onClick)
}
@OptIn(ExperimentalFoundationApi::class)
internal fun Modifier.softCombinedClickable(onClick:()->Unit,onLongClick:()->Unit)=composed {
 val source=remember{MutableInteractionSource()}
 clip(BridgeShapes.Row).combinedClickable(interactionSource=source,indication=rememberBridgeIndication(),role=Role.Button,onClick=onClick,onLongClick=onLongClick)
}

@Composable internal fun BridgeIconButton(onClick:()->Unit,modifier:Modifier=Modifier,enabled:Boolean=true,active:Boolean=false,filled:Boolean=false,content:@Composable ()->Unit){
 val source=remember{MutableInteractionSource()};val pressed by source.collectIsPressedAsState()
 val scale by animateFloatAsState(if(pressed&&enabled)0.92f else 1f,tween(BridgeMotion.Press),label="icon press")
 val colors=MaterialTheme.colorScheme
 val background by animateColorAsState(if(active)colors.surfaceContainerHigh else colors.surfaceContainerHigh.copy(alpha=0f),tween(BridgeMotion.Release),label="icon active")
 Box(modifier.size(48.dp).clip(CircleShape).background(background).clickable(enabled=enabled,interactionSource=source,indication=rememberBridgeIndication(),role=Role.Button,onClick=onClick),contentAlignment=Alignment.Center){
  val tint=if(filled){if(enabled)colors.surface else colors.onSurfaceVariant.copy(alpha=0.5f)}else if(enabled)colors.onSurface else colors.onSurfaceVariant.copy(alpha=0.45f)
  Box(Modifier.size(if(filled)36.dp else 24.dp).graphicsLayer{scaleX=scale;scaleY=scale}.then(if(filled)Modifier.background(if(enabled)colors.onSurface else colors.outlineVariant,CircleShape)else Modifier),contentAlignment=Alignment.Center){CompositionLocalProvider(LocalContentColor provides tint,content=content)}
 }
}
@Composable internal fun BridgeChevron(expanded:Boolean,description:String?,modifier:Modifier=Modifier){
 val angle by animateFloatAsState(if(expanded)90f else 0f,tween(BridgeMotion.Icon,easing=FastOutSlowInEasing),label="disclosure arrow")
 Icon(BridgeIcons.Chevron,description,modifier.size(20.dp).graphicsLayer{rotationZ=angle},tint=MaterialTheme.colorScheme.onSurfaceVariant)
}
@Composable internal fun BridgePressIcon(icon:ImageVector,description:String?,source:InteractionSource,modifier:Modifier=Modifier,tint:Color=LocalContentColor.current){
 val pressed by source.collectIsPressedAsState()
 val scale by animateFloatAsState(if(pressed)0.94f else 1f,tween(if(pressed)BridgeMotion.Press else BridgeMotion.Release),label="action icon press")
 Icon(icon,description,modifier.graphicsLayer{scaleX=scale;scaleY=scale},tint=tint)
}

/** Shared rounded strokes for navigation and actions; no font or bitmap glyphs. */
internal object BridgeIcons {
 private fun icon(name:String,filled:Boolean=false,draw:PathBuilder.()->Unit)=ImageVector.Builder(name,24.dp,24.dp,24f,24f).apply{
  path(fill=if(filled)SolidColor(Color.Black)else null,fillAlpha=0.18f,stroke=SolidColor(Color.Black),strokeLineWidth=1.75f,strokeLineCap=StrokeCap.Round,strokeLineJoin=StrokeJoin.Round,pathBuilder=draw)
 }.build()
 val Menu=icon("Menu"){moveTo(4f,6f);lineTo(20f,6f);moveTo(4f,12f);lineTo(20f,12f);moveTo(4f,18f);lineTo(20f,18f)}
 val Chevron=icon("Chevron"){moveTo(9f,6f);lineTo(15f,12f);lineTo(9f,18f)}
 val Back=icon("Back"){moveTo(15f,6f);lineTo(9f,12f);lineTo(15f,18f)}
 val Add=icon("Add"){moveTo(12f,5f);lineTo(12f,19f);moveTo(5f,12f);lineTo(19f,12f)}
 val Minus=icon("Minus"){moveTo(5f,12f);lineTo(19f,12f)}
 val Expand=icon("Expand"){moveTo(9f,5f);lineTo(5f,5f);lineTo(5f,9f);moveTo(15f,5f);lineTo(19f,5f);lineTo(19f,9f);moveTo(5f,15f);lineTo(5f,19f);lineTo(9f,19f);moveTo(19f,15f);lineTo(19f,19f);lineTo(15f,19f)}
 val Eye=icon("Eye"){moveTo(2f,12f);curveTo(6f,4f,18f,4f,22f,12f);curveTo(18f,20f,6f,20f,2f,12f);close();moveTo(15f,12f);arcTo(3f,3f,0f,true,true,9f,12f);arcTo(3f,3f,0f,true,true,15f,12f)}
 val EyeOff=icon("EyeOff"){moveTo(3f,3f);lineTo(21f,21f);moveTo(9f,5.5f);curveTo(14f,4f,19f,7f,22f,12f);curveTo(21f,14f,19.5f,16f,18f,17f);moveTo(6f,7f);curveTo(4f,8.5f,3f,10f,2f,12f);curveTo(5f,18f,11f,20f,16f,18.5f);moveTo(10f,10f);curveTo(7.5f,13f,11f,16.5f,14f,14f)}
 val Close=icon("Close"){moveTo(6f,6f);lineTo(18f,18f);moveTo(18f,6f);lineTo(6f,18f)}
 val More=ImageVector.Builder("More",24.dp,24.dp,24f,24f).apply{path(fill=SolidColor(Color.Black)){for(x in listOf(5f,12f,19f)){moveTo(x+1.6f,12f);arcTo(1.6f,1.6f,0f,true,true,x-1.6f,12f);arcTo(1.6f,1.6f,0f,true,true,x+1.6f,12f);close()}}}.build()
 val Search=icon("Search"){moveTo(17f,10.5f);arcTo(6.5f,6.5f,0f,true,true,4f,10.5f);arcTo(6.5f,6.5f,0f,true,true,17f,10.5f);moveTo(15.5f,15.5f);lineTo(20f,20f)}
 val Computer=icon("Computer"){moveTo(5.5f,4.5f);lineTo(18.5f,4.5f);quadTo(20.5f,4.5f,20.5f,6.5f);lineTo(20.5f,14.5f);quadTo(20.5f,16.5f,18.5f,16.5f);lineTo(5.5f,16.5f);quadTo(3.5f,16.5f,3.5f,14.5f);lineTo(3.5f,6.5f);quadTo(3.5f,4.5f,5.5f,4.5f);close();moveTo(9.5f,16.5f);lineTo(9f,20f);lineTo(15f,20f);lineTo(14.5f,16.5f);moveTo(7.5f,20f);lineTo(16.5f,20f)}
 val Folder=icon("Folder"){moveTo(3.5f,7.5f);quadTo(3.5f,5.5f,5.5f,5.5f);lineTo(8.7f,5.5f);quadTo(9.5f,5.5f,10.1f,6.2f);lineTo(11.2f,7.6f);quadTo(11.5f,8f,12.2f,8f);lineTo(18.5f,8f);quadTo(20.5f,8f,20.5f,10f);lineTo(20.5f,18f);quadTo(20.5f,20f,18.5f,20f);lineTo(5.5f,20f);quadTo(3.5f,20f,3.5f,18f);close()}
 val Home=icon("Home"){moveTo(3f,10f);lineTo(12f,3f);lineTo(21f,10f);moveTo(5f,9f);lineTo(5f,19f);quadTo(5f,21f,7f,21f);lineTo(10f,21f);lineTo(10f,14f);lineTo(14f,14f);lineTo(14f,21f);lineTo(17f,21f);quadTo(19f,21f,19f,19f);lineTo(19f,9f)}
 val Up=icon("Up"){moveTo(12f,20f);lineTo(12f,4f);moveTo(5f,11f);lineTo(12f,4f);lineTo(19f,11f)}
 val Down=icon("Down"){moveTo(12f,4f);lineTo(12f,20f);moveTo(5f,13f);lineTo(12f,20f);lineTo(19f,13f)}
 val Check=icon("Check"){moveTo(5f,12f);lineTo(10f,17f);lineTo(20f,7f)}
 private val starPath:PathBuilder.()->Unit={moveTo(12f,3f);lineTo(14.8f,8.7f);lineTo(21f,9.6f);lineTo(16.5f,14f);lineTo(17.6f,20.2f);lineTo(12f,17.3f);lineTo(6.4f,20.2f);lineTo(7.5f,14f);lineTo(3f,9.6f);lineTo(9.2f,8.7f);close()}
 val Star=icon("Star",draw=starPath)
 val StarFilled=icon("StarFilled",filled=true,draw=starPath)
 private val pinPath:PathBuilder.()->Unit={moveTo(12.56f,5.55f);lineTo(17.47f,8.99f);lineTo(15.18f,12.26f);lineTo(15.34f,17.26f);quadTo(14.77f,18.08f,13.95f,17.51f);lineTo(5.76f,11.77f);quadTo(4.94f,11.20f,5.51f,10.38f);lineTo(10.26f,8.82f);close();moveTo(11.74f,4.97f);lineTo(18.29f,9.56f);moveTo(9.85f,14.64f);lineTo(5.84f,20.37f)}
 val Pin=icon("Pin",draw=pinPath)
 val PinFilled=icon("PinFilled",filled=true,draw=pinPath)
 val Info=icon("Info"){moveTo(21f,12f);arcTo(9f,9f,0f,true,true,3f,12f);arcTo(9f,9f,0f,true,true,21f,12f);moveTo(12f,11f);lineTo(12f,17f);moveTo(12f,7f);lineTo(12f,7.1f)}
 val Delete=icon("Delete"){moveTo(3f,6f);lineTo(21f,6f);moveTo(9f,6f);lineTo(9f,3f);lineTo(15f,3f);lineTo(15f,6f);moveTo(5f,6f);lineTo(6f,20f);quadTo(6f,21f,8f,21f);lineTo(16f,21f);quadTo(18f,21f,18f,20f);lineTo(19f,6f);moveTo(10f,10f);lineTo(10f,17f);moveTo(14f,10f);lineTo(14f,17f)}
 val Settings=icon("Settings"){moveTo(4f,7f);lineTo(8f,7f);moveTo(12f,7f);lineTo(20f,7f);moveTo(12f,7f);arcTo(2f,2f,0f,true,true,8f,7f);arcTo(2f,2f,0f,true,true,12f,7f);moveTo(4f,17f);lineTo(13f,17f);moveTo(17f,17f);lineTo(20f,17f);moveTo(17f,17f);arcTo(2f,2f,0f,true,true,13f,17f);arcTo(2f,2f,0f,true,true,17f,17f)}
 val Scan=icon("Scan"){moveTo(8f,3f);lineTo(5f,3f);quadTo(3f,3f,3f,5f);lineTo(3f,8f);moveTo(16f,3f);lineTo(19f,3f);quadTo(21f,3f,21f,5f);lineTo(21f,8f);moveTo(21f,16f);lineTo(21f,19f);quadTo(21f,21f,19f,21f);lineTo(16f,21f);moveTo(8f,21f);lineTo(5f,21f);quadTo(3f,21f,3f,19f);lineTo(3f,16f);moveTo(7f,7f);lineTo(10f,7f);lineTo(10f,10f);lineTo(7f,10f);close();moveTo(14f,7f);lineTo(17f,7f);lineTo(17f,10f);lineTo(14f,10f);close();moveTo(7f,14f);lineTo(10f,14f);lineTo(10f,17f);lineTo(7f,17f);close();moveTo(14f,14f);lineTo(14f,17f);lineTo(17f,17f);moveTo(17f,14f);lineTo(17f,14.1f)}
 val Chat=icon("Chat"){moveTo(5f,3f);lineTo(19f,3f);quadTo(21f,3f,21f,5f);lineTo(21f,15f);quadTo(21f,17f,19f,17f);lineTo(9f,17f);lineTo(3f,21f);lineTo(3f,5f);quadTo(3f,3f,5f,3f);moveTo(7f,8f);lineTo(17f,8f);moveTo(7f,12f);lineTo(14f,12f)}
}
