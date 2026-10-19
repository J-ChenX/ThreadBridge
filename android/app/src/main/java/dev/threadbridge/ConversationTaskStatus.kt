package dev.threadbridge

import androidx.compose.foundation.layout.size
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp

internal fun taskIsRunning(thread:ThreadRow?)=thread?.online==true && thread.taskStatus in setOf("running","active","inProgress")

@Composable internal fun ConversationTaskStatus(thread:ThreadRow?,modifier:Modifier=Modifier) {
 if(taskIsRunning(thread))CircularProgressIndicator(modifier.size(18.dp).semantics{contentDescription="对话正在进行"},strokeWidth=2.dp,color=MaterialTheme.colorScheme.onSurfaceVariant)
}
