package dev.threadbridge

import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.layout
import androidx.compose.ui.unit.constrainHeight
import androidx.compose.ui.unit.constrainWidth
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.offset
import kotlin.math.roundToInt

/** One concurrent transition for the composer; read its progress in measurement,
 * so each frame resizes only the input chrome instead of recomposing the chat.
 */
@Composable internal fun rememberComposerMotion(focused:Boolean):()->Float {
 val progress=animateFloatAsState(if(focused)1f else 0f,tween(160,easing=FastOutSlowInEasing),label="composer")
 return remember(progress){{progress.value}}
}

internal fun Modifier.composerMargins(progress:()->Float)=layout { measurable,constraints->
 val fraction=progress().coerceIn(0f,1f)
 val side=(32.dp.toPx()+(12.dp.toPx()-32.dp.toPx())*fraction).roundToInt()
 val top=6.dp.roundToPx()
 val bottom=(32.dp.toPx()+(12.dp.toPx()-32.dp.toPx())*fraction).roundToInt()
 val child=measurable.measure(constraints.offset(-side*2,-top-bottom))
 layout(constraints.constrainWidth(child.width+side*2),constraints.constrainHeight(child.height+top+bottom)) {
  child.placeRelative(side,top)
 }
}

internal fun Modifier.composerBodyFrame(progress:()->Float)=layout { measurable,constraints->
 val fraction=progress().coerceIn(0f,1f)
 val minimum=(52.dp.toPx()+(100.dp.toPx()-52.dp.toPx())*fraction).roundToInt()
 val top=(2.dp.toPx()+(16.dp.toPx()-2.dp.toPx())*fraction).roundToInt()
 val bottom=(2.dp.toPx()+(8.dp.toPx()-2.dp.toPx())*fraction).roundToInt();val start=18.dp.roundToPx();val end=8.dp.roundToPx()
 val outer=constraints.copy(minHeight=maxOf(constraints.minHeight,minOf(minimum,constraints.maxHeight)))
 val child=measurable.measure(outer.offset(-start-end,-top-bottom))
 layout(outer.constrainWidth(child.width+start+end),outer.constrainHeight(child.height+top+bottom)) {
  child.placeRelative(start,top)
 }
}
