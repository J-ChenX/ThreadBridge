package dev.threadbridge

import androidx.compose.animation.Crossfade
import androidx.compose.animation.core.tween
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

@Composable internal fun ConnectionKeyField(value:String,change:(String)->Unit,visible:Boolean,toggle:()->Unit,busy:Boolean,reuse:Boolean){
 OutlinedTextField(value=value,onValueChange={change(it.take(4096))},label={Text("连接密钥")},
  placeholder={if(reuse)Text("留空使用当前密钥",fontSize=14.sp)},enabled=!busy,singleLine=true,
  visualTransformation=if(visible)VisualTransformation.None else PasswordVisualTransformation(),
  keyboardOptions=KeyboardOptions(autoCorrectEnabled=false,keyboardType=KeyboardType.Password,imeAction=ImeAction.Done),
  trailingIcon={BridgeIconButton(enabled=!busy,onClick=toggle){Crossfade(visible,animationSpec=tween(BridgeMotion.Icon),label="keyVisibility"){show->Icon(if(show)BridgeIcons.EyeOff else BridgeIcons.Eye,if(show)"隐藏连接密钥"else"显示连接密钥")}}},
  modifier=Modifier.fillMaxWidth().testTag("connection_key"),shape=BridgeShapes.Field)
}

@Composable internal fun ConnectionSettingsDialog(repo:Repository,dismiss:()->Unit,rePair:()->Unit){
 val scope=rememberCoroutineScope();val focus=LocalFocusManager.current
 var server by remember{mutableStateOf(repo.credentials.server)}
 // Keep entered secrets out of saveable state and never prefill the saved key.
 var key by remember{mutableStateOf("")};var visible by remember{mutableStateOf(false)}
 var lan by remember{mutableStateOf(repo.credentials.lan)};var busy by remember{mutableStateOf(false)}
 var error by remember{mutableStateOf<String?>(null)};var confirmReset by remember{mutableStateOf(false)}
 val addressError=if(server.isNotBlank())runCatching{ConnectionConfiguration.server(server,lan)}.exceptionOrNull()?.message else null
 AlertDialog(shape=BridgeShapes.Panel,onDismissRequest={if(!busy)dismiss()},title={Text("连接与设置")},
  modifier=Modifier.imePadding(),text={Column(Modifier.verticalScroll(rememberScrollState()),verticalArrangement=Arrangement.spacedBy(16.dp)){
   Text("修改地址或更新密钥，验证成功后保存。",fontSize=14.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)
   OutlinedTextField(value=server,onValueChange={server=it;error=null},label={Text("服务器地址")},singleLine=true,enabled=!busy,
    isError=addressError!=null,supportingText=if(addressError==null)null else ({Text(addressError)}),shape=BridgeShapes.Field,
    keyboardOptions=KeyboardOptions(autoCorrectEnabled=false,keyboardType=KeyboardType.Uri,imeAction=ImeAction.Next),modifier=Modifier.fillMaxWidth().testTag("connection_server"))
   ConnectionKeyField(key,{key=it;error=null},visible,{visible=!visible},busy,true)
   Text("留空会保留当前密钥。请使用 Hub 的手机访问密钥，非 cpolar 令牌。",fontSize=12.sp,lineHeight=18.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)
   Row(Modifier.fillMaxWidth(),verticalAlignment=androidx.compose.ui.Alignment.CenterVertically){Text("局域网测试",fontSize=14.sp,modifier=Modifier.weight(1f));Switch(lan,{lan=it;error=null},enabled=!busy)}
   if(error!=null)Text(error!!,color=MaterialTheme.colorScheme.error,fontSize=13.sp)
   Text("地址和密钥验证失败时保留原配置。更换同一服务的入口会保留对话、草稿和收藏。",fontSize=12.sp,lineHeight=18.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)
   TextButton(enabled=!busy,onClick={confirmReset=true}){Text("清除并重新配对",fontSize=13.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)}
  }},confirmButton={TextButton(enabled=!busy&&server.isNotBlank()&&addressError==null,onClick={focus.clearFocus();busy=true;error=null;scope.launch{try{repo.configureConnection(server,key,lan);key="";dismiss()}catch(e:CancellationException){throw e}catch(e:Exception){error=e.message?:"连接验证失败，请稍后重试"}finally{busy=false}}}){if(busy){CircularProgressIndicator(Modifier.size(16.dp),strokeWidth=2.dp);Spacer(Modifier.width(8.dp))};Text(if(busy)"正在验证…"else"验证并保存")}},
  dismissButton={TextButton(enabled=!busy,onClick=dismiss){Text("取消")}})
 if(confirmReset)AlertDialog(shape=BridgeShapes.Panel,onDismissRequest={confirmReset=false},title={Text("重新配对？")},text={Text("这会清除本机缓存、草稿及收藏。电脑上的 Codex 对话仍会保留。")},confirmButton={TextButton(onClick={confirmReset=false;rePair()}){Text("清除本机并重新配对")}},dismissButton={TextButton(onClick={confirmReset=false}){Text("取消")}})
}
