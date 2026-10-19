package dev.threadbridge

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle

/** Questions stay expanded in the timeline, separate from collapsible process messages. */
@Composable internal fun QuestionCard(repo:Repository,message:MessageRow,work:((suspend()->Unit))->Unit) {
 val question=remember(message.version){QuestionPresentation.parse(message)}
 if(question==null){Text("问题暂时无法解析，请在电脑端查看",color=MaterialTheme.colorScheme.error);return}
 val thread by remember(message.threadId){repo.dao.threadFlow(message.threadId)}.collectAsStateWithLifecycle(initialValue=null)
 val pending by remember(message.threadId){repo.dao.pending(message.threadId)}.collectAsStateWithLifecycle(initialValue=null)
 val connection by repo.connection.collectAsStateWithLifecycle()
 var answer by rememberSaveable(message.threadId,message.id){mutableStateOf("")}
 var submitting by remember(message.id){mutableStateOf(false)}
 val related=pending?.takeIf{QuestionPresentation.matchesReply(it.payload,question.id)}
 val blocked=submitting||pending?.status in setOf("submitting","accepted","dispatching","upstream_queued","unknown")
 val online=connection.startsWith("已连接")&&thread?.online==true&&thread?.canSend==true
 Surface(shape=BridgeShapes.Panel,color=MaterialTheme.colorScheme.surfaceContainerLow,modifier=Modifier.fillMaxWidth().testTag("question_card")) {
  Column(Modifier.padding(16.dp),verticalArrangement=Arrangement.spacedBy(12.dp)) {
   Text("问题 · ${QuestionPresentation.statusLabel(question.status)}",fontSize=13.sp,fontWeight=FontWeight.Medium,color=MaterialTheme.colorScheme.primary)
   Text(question.title,fontSize=16.sp,lineHeight=25.sp)
   if(question.status=="answered")Text(question.answer?.let{if(it.isBlank())"已跳过"else"回答：$it"}?:"回答已在电脑端处理",fontSize=14.sp,lineHeight=22.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)
   if(question.canReply){
    question.options.forEach { option->
     OutlinedButton(onClick={answer=option.label},enabled=!blocked,modifier=Modifier.fillMaxWidth().heightIn(min=48.dp)) {
      Column(Modifier.fillMaxWidth(),verticalArrangement=Arrangement.spacedBy(4.dp)){
       Text(if(answer==option.label)"已选：${option.label}"else option.label)
       if(option.description.isNotBlank())Text(option.description,fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)
      }
     }
    }
    OutlinedTextField(value=answer,onValueChange={if(it.toByteArray().size<=8000)answer=it},label={Text("回答")},placeholder={Text("选择选项或输入回答")},enabled=!blocked,modifier=Modifier.fillMaxWidth().testTag("question_answer"),minLines=2,maxLines=6)
    related?.let{receipt->
     Text(ReceiptPresentation.label(receipt.status,true),fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)
     receipt.error?.let{Text(ReceiptPresentation.errorLabel(it),fontSize=12.sp,color=MaterialTheme.colorScheme.error)}
    }
    Text(if(blocked)"有消息正在提交，请等待电脑确认后再回答。"else if(!online)"电脑连接恢复后可提交回答，输入会保留。"else "回答将发送到原对话，桌面队列可能需要等待。",fontSize=12.sp,lineHeight=18.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)
    Button(enabled=online&&!blocked&&answer.isNotBlank(),onClick={
     submitting=true
     work{try{repo.answerQuestion(message,answer)}finally{submitting=false}}
    },modifier=Modifier.fillMaxWidth().heightIn(min=48.dp).testTag("question_submit")){Text(if(submitting)"正在提交…"else"提交回答")}
   }else if(question.kind=="blocking"&&question.status=="pending")Text("此问题需要在电脑端回答。",fontSize=13.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)
  }
 }
}
