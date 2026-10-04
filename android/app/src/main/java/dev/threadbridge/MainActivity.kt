package dev.threadbridge

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.compose.foundation.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.outlined.*
import androidx.compose.material3.*
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.testTagsAsResourceId
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.input.OffsetMapping
import androidx.compose.ui.text.input.TransformedText
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.view.WindowCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.collect
import org.json.JSONObject

class MainActivity:ComponentActivity() {
 private lateinit var repo:Repository
 private val destination=mutableStateOf<String?>(null)
 override fun onCreate(savedInstanceState:Bundle?) {
  super.onCreate(savedInstanceState);enableEdgeToEdge();repo=Repository.get(applicationContext);handle(intent)
  lifecycleScope.launch { repeatOnLifecycle(Lifecycle.State.STARTED) { repo.connect(this);try{awaitCancellation()}finally{repo.close()} } }
  setContent {
   val dark=isSystemInDarkTheme()
   val colors=if(dark)darkColorScheme(primary=Color(0xFFF5F5F5),onPrimary=Color(0xFF171717),background=Color(0xFF212121),surface=Color(0xFF212121),surfaceContainerLow=Color(0xFF2F2F2F),surfaceContainer=Color(0xFF2F2F2F),outline=Color(0xFF666666),surfaceContainerHigh=Color(0xFF383838),onSurface=Color(0xFFF0F0F0),onSurfaceVariant=Color(0xFFA9A9A9),outlineVariant=Color(0xFF404040))
    else lightColorScheme(primary=Color(0xFF171717),onPrimary=Color.White,background=Color.White,surface=Color.White,surfaceContainerLow=Color(0xFFF5F5F5),surfaceContainer=Color(0xFFF5F5F5),outline=Color(0xFFB7B7B7),surfaceContainerHigh=Color(0xFFEEEEEE),onSurface=Color(0xFF171717),onSurfaceVariant=Color(0xFF757575),outlineVariant=Color(0xFFE8E8E8))
   SideEffect { WindowCompat.getInsetsController(window,window.decorView).apply { isAppearanceLightStatusBars=!dark;isAppearanceLightNavigationBars=!dark } }
   MaterialTheme(colorScheme=colors) { App(repo,destination.value){destination.value=it} }
  }
 }
 override fun onNewIntent(intent:Intent){super.onNewIntent(intent);handle(intent)}
 private fun handle(intent:Intent?){val uri=intent?.data;if(uri?.scheme=="threadbridge"&&uri.host=="thread"){val id=uri.lastPathSegment;if(id!=null&&id.matches(Regex("[a-f0-9]{64}")))destination.value=id}}
}
private fun stateLabel(status:String)=when(status) {
 "resume_ready","queue_ready","idle"->"可以继续对话"
 "capture_only"->"只读对话";"completed"->"已完成";"active","inProgress"->"进行中"
 "failed"->"执行失败";"waiting_input"->"需要在电脑处理";"interrupted"->"已中断";"notLoaded"->"任务未加载";else->"等待电脑更新"
}
@OptIn(ExperimentalMaterial3Api::class,ExperimentalFoundationApi::class)
@Composable fun App(repo:Repository,destination:String?,navigate:(String?)->Unit) {
 val scope=rememberCoroutineScope();val focus=LocalFocusManager.current;val drawer=rememberDrawerState(DrawerValue.Closed)
 var paired by remember{mutableStateOf(repo.credentials.token().isNotEmpty())}
 val threads by repo.dao.threads().collectAsStateWithLifecycle(initialValue=emptyList())
 val hosts by repo.dao.hosts().collectAsStateWithLifecycle(initialValue=emptyList())
 val epoch by repo.collection.collectAsStateWithLifecycle()
 val unread by repo.readState.unread.collectAsStateWithLifecycle()
 LaunchedEffect(threads){repo.readState.restore(threads)}
 var expandedDevices by androidx.compose.runtime.saveable.rememberSaveable{mutableStateOf(listOf<String>())}
 var collapsed by androidx.compose.runtime.saveable.rememberSaveable{mutableStateOf(listOf<String>())}
 var deleteTarget by remember{mutableStateOf<ThreadRow?>(null)}
 var confirmDelete by remember{mutableStateOf<ThreadRow?>(null)}
 val connection by repo.connection.collectAsStateWithLifecycle();val warning by repo.captureWarning.collectAsStateWithLifecycle()
 var error by remember{mutableStateOf<String?>(null)};var busy by remember{mutableStateOf(false)};var refreshing by remember{mutableStateOf(false)}
 var settings by remember{mutableStateOf(false)};var details by remember{mutableStateOf(false)};var menu by remember{mutableStateOf(false)}
 fun work(block:suspend()->Unit){scope.launch{busy=true;try{block()}catch(e:CancellationException){throw e}catch(e:Exception){error=e.message?:"操作失败，请稍后重试"}finally{busy=false}}}
 fun refresh(){if(refreshing)return;refreshing=true;scope.launch{try{repo.sync()}catch(e:CancellationException){throw e}catch(e:Exception){error=e.message?:"同步失败，请稍后重试"}finally{refreshing=false}}}
 fun open(id:String?){threads.find{it.id==id}?.let{repo.readState.markRead(it)};focus.clearFocus();navigate(id);scope.launch{drawer.close()}}
 LaunchedEffect(destination,paired){repo.selected=destination;if(paired)runCatching{repo.sync()}}
 BackHandler(paired&&(drawer.isOpen||destination!=null)){if(drawer.isOpen)scope.launch{drawer.close()}else navigate(null)}
 LaunchedEffect(epoch){if(epoch.isNotEmpty()&&destination!=null&&repo.selected==null)navigate(null)}
 val current=threads.find{it.id==destination}
 fun expand(id:String){expandedDevices=if(id in expandedDevices)expandedDevices-id else expandedDevices+id}
 fun toggle(id:String){collapsed=if(id in collapsed)collapsed-id else collapsed+id}
 ModalNavigationDrawer(drawerState=drawer,gesturesEnabled=paired,drawerContent={
  if(paired)ModalDrawerSheet(drawerContainerColor=MaterialTheme.colorScheme.surfaceContainerLow,modifier=Modifier.width(300.dp)) {
   Row(Modifier.fillMaxWidth().padding(start=20.dp,end=10.dp,top=14.dp,bottom=10.dp),verticalAlignment=Alignment.CenterVertically){Text("续桥",fontSize=22.sp,fontWeight=FontWeight.SemiBold);Spacer(Modifier.weight(1f));IconButton(onClick={scope.launch{drawer.close()}}){Icon(Icons.Outlined.ChevronLeft,"收起对话列表")}}
   Text("设备",fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.padding(horizontal=20.dp,vertical=12.dp))
   LazyColumn(Modifier.weight(1f),contentPadding=PaddingValues(horizontal=10.dp)){
    deviceItems(hosts,threads,collapsed,destination,::toggle,{open(it)},{deleteTarget=it},unread,expandedDevices,::expand,true)
   }
   HorizontalDivider(color=MaterialTheme.colorScheme.outlineVariant)
   Row(Modifier.fillMaxWidth().clickable{scope.launch{drawer.close()};settings=true}.padding(20.dp),verticalAlignment=Alignment.CenterVertically){Icon(Icons.Outlined.Settings,null,Modifier.size(20.dp));Spacer(Modifier.width(12.dp));Column{Text("连接与设置",fontSize=14.sp);Text(connection,fontSize=11.sp,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.padding(top=4.dp))}}
  }
 }) {
  val navigationButton: @Composable ()->Unit={ConversationIconButton(floating=destination!=null,onClick={focus.clearFocus();scope.launch{drawer.open()}}){Icon(Icons.Outlined.Menu,"打开对话列表")}}
  val moreButton: @Composable ()->Unit={Box{ConversationIconButton(floating=destination!=null,onClick={focus.clearFocus();menu=true}){Icon(Icons.Outlined.MoreHoriz,"更多选项")};DropdownMenu(menu,{menu=false},modifier=Modifier.width(224.dp),shape=RoundedCornerShape(20.dp),containerColor=MaterialTheme.colorScheme.surfaceContainerLow,tonalElevation=0.dp,shadowElevation=6.dp){
    if(destination!=null)DropdownMenuItem(text={Text(current?.let{displayTitle(it,hosts)}?:"对话",fontSize=13.sp,maxLines=2,overflow=TextOverflow.Ellipsis)},enabled=false,onClick={})
    DropdownMenuItem(text={Text("连接状态",fontSize=14.sp)},leadingIcon={Icon(Icons.Outlined.Info,null,Modifier.size(20.dp))},onClick={menu=false;details=true})
    if(destination==null)DropdownMenuItem(text={Text("设置",fontSize=14.sp)},leadingIcon={Icon(Icons.Outlined.Settings,null,Modifier.size(20.dp))},onClick={menu=false;settings=true})
   }}}
  Scaffold(containerColor=MaterialTheme.colorScheme.background,
   contentWindowInsets=if(paired&&destination!=null)WindowInsets(0,0,0,0)else ScaffoldDefaults.contentWindowInsets,
   topBar={if(paired&&destination==null)TopAppBar(
    title={Text("设备列表",fontSize=18.sp,fontWeight=FontWeight.SemiBold)},
    navigationIcon=navigationButton,actions={moreButton()},
    colors=TopAppBarDefaults.topAppBarColors(containerColor=MaterialTheme.colorScheme.background))}) { padding->
   Box(Modifier.fillMaxSize().padding(padding).consumeWindowInsets(padding)) {
    if(!paired)PairScreen(busy,error){server,code,lan->work{repo.pair(server,code,lan);paired=true;error=null}}
    else Column(Modifier.fillMaxSize()) {
     if(destination==null&&warning!=null)Row(Modifier.fillMaxWidth().clickable{details=true}.padding(horizontal=20.dp,vertical=8.dp),verticalAlignment=Alignment.CenterVertically){Icon(Icons.Outlined.Info,null,tint=MaterialTheme.colorScheme.error,modifier=Modifier.size(14.dp));Spacer(Modifier.width(7.dp));Text(CapturePresentation.summary(warning!!),fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant);Spacer(Modifier.weight(1f));Text("查看",fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)}
     if(destination==null)ConversationList(hosts,threads,collapsed,::toggle,{open(it)},{deleteTarget=it},unread,expandedDevices,::expand,refreshing,::refresh)
     else key(destination,epoch){ChatScreen(repo,destination,current,busy,refreshing,::refresh,header={ConversationHeader(navigationButton,moreButton,warning){details=true}},work={work(it)})}
    }
    if(busy)LinearProgressIndicator(Modifier.fillMaxWidth().height(2.dp).align(Alignment.TopCenter),color=MaterialTheme.colorScheme.onSurface)
   }
  }
 }
 if(deleteTarget!=null)ModalBottomSheet(onDismissRequest={deleteTarget=null},containerColor=MaterialTheme.colorScheme.surface){
  Text(displayTitle(deleteTarget!!,hosts),fontSize=16.sp,fontWeight=FontWeight.SemiBold,maxLines=2,overflow=TextOverflow.Ellipsis,modifier=Modifier.padding(horizontal=24.dp,vertical=12.dp))
  Row(Modifier.fillMaxWidth().clickable{confirmDelete=deleteTarget;deleteTarget=null}.padding(24.dp),verticalAlignment=Alignment.CenterVertically){Icon(Icons.Outlined.Delete,null,tint=MaterialTheme.colorScheme.error);Spacer(Modifier.width(16.dp));Text("删除对话",color=MaterialTheme.colorScheme.error)}
  Spacer(Modifier.height(12.dp))
 }
 if(confirmDelete!=null)AlertDialog(onDismissRequest={confirmDelete=null},title={Text("删除这个对话？")},text={Text("将删除手机服务中的同步副本，并停止同步此对话。电脑 Codex 中的原始对话会保留。")},confirmButton={TextButton(onClick={val target=confirmDelete!!;work{repo.deleteThread(target.id);if(destination==target.id)open(null);confirmDelete=null}}){Text("删除",color=MaterialTheme.colorScheme.error)}},dismissButton={TextButton(onClick={confirmDelete=null}){Text("取消")}})
 if(error!=null&&paired)AlertDialog(onDismissRequest={error=null},title={Text("暂时无法完成")},text={Text(error!!)},confirmButton={TextButton(onClick={error=null}){Text("知道了")}})
 if(details)AlertDialog(onDismissRequest={details=false},title={Text("连接状态")},text={Column(verticalArrangement=Arrangement.spacedBy(14.dp)){Text(connection);current?.let{Text(stateLabel(it.status));if(it.status in listOf("queue_ready","capture_only","resume_ready"))Text("显示已同步的用户文字、图片、过程说明与最终回复；尚未同步的历史请在电脑查看。",fontSize=13.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)};if(warning!=null)Text(warning!!);Text("已配对电脑：${hosts.size} 台\n已保存对话：${threads.size} 个",fontSize=13.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)}},confirmButton={TextButton(onClick={details=false}){Text("关闭")}})
 if(settings)AlertDialog(onDismissRequest={settings=false},title={Text("连接与设置")},text={Column(verticalArrangement=Arrangement.spacedBy(16.dp)){Text(repo.credentials.server,fontSize=14.sp);Text("凭证由 Android Keystore 加密保护。点击现有 ntfy 通知可回到对话。",fontSize=14.sp);Text("重新配对会清除本机缓存和草稿，请先保存需要的内容。",fontSize=13.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)}},confirmButton={TextButton(onClick={settings=false}){Text("关闭")}},dismissButton={TextButton(onClick={work{repo.forget();paired=false;navigate(null);settings=false}}){Text("清除并重新配对")}})
}
private fun displayTitle(thread:ThreadRow,hosts:List<HostRow>):String {
 val name=hosts.find{it.id==thread.hostId}?.name?:return thread.title
 return thread.title.removePrefix("$name · ").removePrefix("windows · ")
}
private fun hostLabel(host:HostRow)=if(host.name.equals("windows",true))"Windows"else host.name
@OptIn(ExperimentalFoundationApi::class)
private fun LazyListScope.deviceItems(hosts:List<HostRow>,threads:List<ThreadRow>,collapsed:List<String>,selected:String?,toggle:(String)->Unit,open:(String)->Unit,delete:(ThreadRow)->Unit,unread:Set<String>,expanded:List<String>,expand:(String)->Unit,inDrawer:Boolean,isPinned:(String)->Boolean={false}){
 for((position,device) in deviceGroups(hosts,threads,unread).withIndex()){
  if(position>0)item(key="device_gap:${device.host.id}"){Spacer(Modifier.height(12.dp))}
  val host=device.host;val group=device.threads
  stickyHeader(key="device:${host.id}"){
   val background=if(inDrawer)MaterialTheme.colorScheme.surfaceContainerLow else MaterialTheme.colorScheme.background
   // Extend a drawing-only fade over the rows passing under a pinned device.
   // Keep it outside a clipped Surface and outside measured item height.
   Box(Modifier.fillMaxWidth().drawWithCache{
    val fadeHeight=28.dp.toPx()
    val fade=Brush.verticalGradient(0f to background,0.35f to background.copy(alpha=0.85f),1f to background.copy(alpha=0f),startY=size.height,endY=size.height+fadeHeight)
    onDrawWithContent{drawContent();if(isPinned(host.id))drawRect(fade,topLeft=Offset(0f,size.height),size=Size(size.width,fadeHeight))}
   }){
   Surface(color=background){
    Row(Modifier.fillMaxWidth().clickable{toggle(host.id)}.heightIn(min=48.dp).padding(horizontal=12.dp,vertical=10.dp),verticalAlignment=Alignment.CenterVertically){
     Icon(Icons.Outlined.Computer,null,Modifier.size(18.dp),tint=MaterialTheme.colorScheme.onSurfaceVariant);Spacer(Modifier.width(12.dp))
     Text(hostLabel(host),fontSize=15.sp,lineHeight=20.sp,fontWeight=FontWeight.SemiBold,modifier=Modifier.weight(1f))
     if(device.unread)UnreadDot("未读设备 ${host.name}")
     Spacer(Modifier.width(10.dp));Text("${group.size}",fontSize=11.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)
     Icon(if(host.id in collapsed)Icons.Outlined.ChevronRight else Icons.Outlined.ExpandMore,if(host.id in collapsed)"展开设备 ${host.name}"else"折叠设备 ${host.name}",Modifier.padding(start=6.dp).size(18.dp))
    }
   }
   }
  }
  if(host.id !in collapsed){
   if(group.isEmpty())item(key="empty:${host.id}"){Text("暂无新对话",fontSize=13.sp,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.padding(start=44.dp,top=4.dp,bottom=16.dp))}
   val visible=if(host.id in expanded)group else device.preview
   items(visible,key={it.id}){thread->Surface(color=if(thread.id==selected)MaterialTheme.colorScheme.surfaceContainerHigh else Color.Transparent,shape=RoundedCornerShape(12.dp),modifier=Modifier.fillMaxWidth().combinedClickable(onClick={open(thread.id)},onLongClick={delete(thread)})){
    Row(Modifier.heightIn(min=44.dp).padding(start=44.dp,end=12.dp,top=8.dp,bottom=8.dp),verticalAlignment=Alignment.CenterVertically){
     Text(displayTitle(thread,hosts),maxLines=1,overflow=TextOverflow.Ellipsis,fontSize=14.sp,lineHeight=20.sp,fontWeight=if(thread.id in unread)FontWeight.Medium else FontWeight.Normal,modifier=Modifier.weight(1f))
     if(thread.id in unread){Spacer(Modifier.width(12.dp));UnreadDot("未读对话 ${thread.title}")}
    }
   }}
   if(group.size>device.preview.size)item(key="expand:${host.id}"){
    TextButton(onClick={expand(host.id)},modifier=Modifier.padding(start=32.dp),contentPadding=PaddingValues(horizontal=12.dp,vertical=4.dp)){
     Text(if(host.id in expanded)"收起"else"展开更多（${group.size-device.preview.size}）",fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)
    }
   }
  }
 }
}
@Composable private fun UnreadDot(description:String){Box(Modifier.size(6.dp).background(Color(0xFF2383E2),CircleShape).semantics{contentDescription=description})}
@OptIn(ExperimentalMaterial3Api::class)
@Composable private fun ConversationList(hosts:List<HostRow>,threads:List<ThreadRow>,collapsed:List<String>,toggle:(String)->Unit,open:(String)->Unit,delete:(ThreadRow)->Unit,unread:Set<String>,expanded:List<String>,expand:(String)->Unit,refreshing:Boolean,refresh:()->Unit){
 val list=rememberLazyListState()
 var keepAtStart by remember{mutableStateOf(true)}
 LaunchedEffect(list){snapshotFlow{list.isScrollInProgress}.collect{scrolling->keepAtStart=if(scrolling)false else !list.canScrollBackward}}
 // Key anchoring can otherwise retain the former first device after a newer
 // group moves above it. Keep an idle overview at the beginning; preserve
 // the anchor only when the user has actually scrolled into the list.
 LaunchedEffect(threads){if(keepAtStart)list.requestScrollToItem(0)}
 PullToRefreshBox(isRefreshing=refreshing,onRefresh={keepAtStart=true;list.requestScrollToItem(0);refresh()},modifier=Modifier.fillMaxSize()){
 LazyColumn(Modifier.fillMaxSize(),state=list,contentPadding=PaddingValues(horizontal=12.dp,vertical=16.dp)){
  if(threads.isEmpty())item{Text("新建的电脑对话会显示在对应设备下",fontSize=13.sp,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.padding(horizontal=12.dp,vertical=8.dp))}
  deviceItems(hosts,threads,collapsed,null,toggle,open,delete,unread,expanded,expand,false){hostId->list.canScrollBackward&&list.layoutInfo.visibleItemsInfo.any{it.key=="device:$hostId"&&it.offset<=0}}
 }
}
}
@Composable private fun PairScreen(busy:Boolean,error:String?,pair:(String,String,Boolean)->Unit) {
 var server by rememberSaveableCompat(BuildConfig.PUBLIC_TEST_SERVER);var code by rememberSaveableCompat("");var lan by remember{mutableStateOf(false)};var scanError by remember{mutableStateOf<String?>(null)}
 val scanner=rememberLauncherForActivityResult(ScanContract()){result->if(result.contents!=null)try{val value=JSONObject(result.contents);server=value.getString("server");code=value.getString("code");lan=server.startsWith("http://")}catch(_:Exception){scanError="这不是续桥配对二维码"}}
 LazyColumn(Modifier.fillMaxSize().imePadding(),contentPadding=PaddingValues(horizontal=28.dp,vertical=50.dp),verticalArrangement=Arrangement.spacedBy(20.dp)) {
  item{Icon(Icons.Outlined.Forum,null,Modifier.size(38.dp));Text("连接你的电脑",fontSize=30.sp,fontWeight=FontWeight.SemiBold,modifier=Modifier.padding(top=28.dp));Text("输入配对码，让对话继续。",fontSize=15.sp,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.padding(top=12.dp))}
  item{OutlinedTextField(server,{server=it},label={Text("服务器地址")},singleLine=true,modifier=Modifier.fillMaxWidth(),shape=RoundedCornerShape(16.dp))}
  item{OutlinedTextField(code,{code=it},label={Text("一次性配对码")},minLines=2,maxLines=3,modifier=Modifier.fillMaxWidth(),shape=RoundedCornerShape(16.dp))}
  item{Row(verticalAlignment=Alignment.CenterVertically){Switch(lan,{lan=it});Spacer(Modifier.width(12.dp));Text("局域网测试",fontSize=14.sp)}}
  item{Button(onClick={pair(server,code,lan)},enabled=!busy&&server.isNotBlank()&&code.isNotBlank(),modifier=Modifier.fillMaxWidth().height(52.dp),shape=RoundedCornerShape(26.dp)){Text(if(busy)"正在连接…"else"连接",fontSize=16.sp)}}
  item{OutlinedButton(onClick={scanner.launch(ScanOptions().setDesiredBarcodeFormats(ScanOptions.QR_CODE).setPrompt("扫描电脑上的配对码").setBeepEnabled(false))},modifier=Modifier.fillMaxWidth().height(52.dp),shape=RoundedCornerShape(26.dp)){Icon(Icons.Outlined.QrCodeScanner,null);Spacer(Modifier.width(10.dp));Text("扫描二维码")}}
  if(error!=null||scanError!=null)item{Text(error?:scanError!!,color=MaterialTheme.colorScheme.error,fontSize=13.sp)}
  item{Text("配对码 5 分钟后过期，只能使用一次。\n凭证保存在你的手机上。",fontSize=12.sp,lineHeight=20.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)}
 }
}
// Keep the same multiline IME configuration when focus changes. Flatten only
// the collapsed preview, preserving every draft character and cursor offset.
private val CompactDraftTransformation=VisualTransformation { text->TransformedText(AnnotatedString(text.text.replace('\n',' ')),OffsetMapping.Identity) }
@Composable private fun rememberSaveableCompat(initial:String)=androidx.compose.runtime.saveable.rememberSaveable{mutableStateOf(initial)}
@OptIn(ExperimentalLayoutApi::class)
@Composable private fun ChatScreen(repo:Repository,id:String,thread:ThreadRow?,busy:Boolean,refreshing:Boolean,refresh:()->Unit,header:@Composable ()->Unit,work:((suspend()->Unit))->Unit) {
 var before by remember(id){mutableLongStateOf(Long.MAX_VALUE)};var beforeId by remember(id){mutableStateOf("")}
 var visibleLimit by remember(id){mutableIntStateOf(100)}
 var nextPage by remember(id){mutableStateOf<MessageCursor?>(null)};var loadingHistory by remember(id){mutableStateOf(false)}
 var ready by remember(id){mutableStateOf(false)};var historyError by remember(id){mutableStateOf(false)}
 val flow=remember(id,before,beforeId,visibleLimit){repo.dao.messages(id,before,beforeId,visibleLimit)}
 val messages by flow.collectAsStateWithLifecycle(initialValue=emptyList())
 val pendingFlow=remember(id){repo.dao.pending(id)};val pending by pendingFlow.collectAsStateWithLifecycle(initialValue=null)
 val list=rememberLazyListState();val scope=rememberCoroutineScope();val focus=LocalFocusManager.current
 var draft by remember(id){mutableStateOf("")};var draftReady by remember(id){mutableStateOf(false)}
 var composerFocused by remember(id){mutableStateOf(false)}
 val composerMotion=rememberComposerMotion(composerFocused)
 // The target changes at the start of dismissal; do not wait for the final inset frame.
 val keyboardTargetVisible=WindowInsets.imeAnimationTarget.getBottom(androidx.compose.ui.platform.LocalDensity.current)>0
 var keyboardWasVisible by remember(id){mutableStateOf(false)}
 LaunchedEffect(keyboardTargetVisible){if(keyboardTargetVisible)keyboardWasVisible=true else if(keyboardWasVisible){focus.clearFocus();keyboardWasVisible=false}}
 BackHandler(composerFocused){focus.clearFocus()}
 val connection by repo.connection.collectAsStateWithLifecycle();val connected=connection.startsWith("已连接")
 val blocked=pending?.status in listOf("submitting","accepted","dispatching","upstream_queued","unknown")
 val captureMode=thread?.status in listOf("queue_ready","capture_only","resume_ready")
 val atLatest by remember{derivedStateOf{before==Long.MAX_VALUE&&list.firstVisibleItemIndex==0&&list.firstVisibleItemScrollOffset==0}}
 LaunchedEffect(thread?.updated,thread?.revision,thread?.messageRevision,atLatest,ready){if(ready&&atLatest)thread?.let{repo.readState.markRead(it)}}
 fun older(){val cursor=nextPage?:return;if(loadingHistory)return;scope.launch{loadingHistory=true;historyError=false;try{nextPage=repo.loadMessages(id,cursor.ordinal,cursor.id);if(visibleLimit>=5000){before=cursor.ordinal;beforeId=cursor.id;visibleLimit=100}else visibleLimit=minOf(5000,visibleLimit+30)}catch(e:CancellationException){throw e}catch(_:Exception){historyError=true}finally{loadingHistory=false}}}
 // Reposition during the next measure without waiting for a network round trip
 // or animating through every paragraph in a long response.
 fun newest(){val changedWindow=before!=Long.MAX_VALUE;if(changedWindow)visibleLimit=100;before=Long.MAX_VALUE;beforeId="";list.requestScrollToItem(0);if(changedWindow)scope.launch{runCatching{nextPage=repo.loadMessages(id)}}}
 fun loadOlder(){
  if(loadingHistory||busy||blocked)return
  if(nextPage!=null)older()
  else if(thread?.historyCursor!=null)work{repo.submit(thread,"",true)}
  else if(historyError)scope.launch{loadingHistory=true;try{nextPage=repo.loadMessages(id);historyError=false}catch(e:CancellationException){throw e}catch(_:Exception){historyError=true}finally{loadingHistory=false}}
 }
 LaunchedEffect(id){draft=repo.draft(id);draftReady=true;try{nextPage=repo.loadMessages(id)}catch(e:CancellationException){throw e}catch(_:Exception){historyError=true}finally{ready=true}}
 LaunchedEffect(pending?.status){if(pending?.status=="history_loaded"){
  // The computer has inserted records older than our cached range. Fetch
  // from that range's boundary while preserving the current reading position.
  val oldest=messages.lastOrNull();loadingHistory=true
  try{nextPage=repo.loadMessages(id,oldest?.ordinal,oldest?.id);visibleLimit=minOf(5000,visibleLimit+30);historyError=false}
  catch(e:CancellationException){throw e}catch(_:Exception){historyError=true}finally{loadingHistory=false}
 }}
 LaunchedEffect(messages.firstOrNull()?.id){if(ready&&atLatest)list.scrollToItem(0)}
 ConversationViewport(list=list,header=header,atLatest=atLatest,newest={newest()},refreshing=refreshing,refresh=refresh,loadingOlder=loadingHistory||(blocked&&runCatching{JSONObject(pending!!.payload).optString("kind")=="history"}.getOrDefault(false)),canLoadOlder=ready&&!busy&&!blocked&&(nextPage!=null||thread?.historyCursor!=null||historyError),loadOlder={loadOlder()},composer={
  Column(Modifier.fillMaxWidth().composerMargins(composerMotion)) {
   Surface(color=MaterialTheme.colorScheme.surface,shape=RoundedCornerShape(26.dp),shadowElevation=2.dp,modifier=Modifier.fillMaxWidth().semantics{testTagsAsResourceId=true}.testTag("conversation_composer")) {
    Box(Modifier.composerBodyFrame(composerMotion)) {
     BasicTextField(value=draft,onValueChange={value->if(value.toByteArray().size<=32000){draft=value;repo.saveDraft(id,value)}},enabled=draftReady&&!busy,visualTransformation=if(composerFocused)VisualTransformation.None else CompactDraftTransformation,maxLines=if(composerFocused)6 else 1,textStyle=TextStyle(color=MaterialTheme.colorScheme.onSurface,fontSize=16.sp,lineHeight=24.sp),cursorBrush=SolidColor(MaterialTheme.colorScheme.onSurface),modifier=Modifier.align(Alignment.TopStart).fillMaxWidth().padding(end=48.dp).heightIn(min=36.dp).onFocusChanged{composerFocused=it.isFocused},decorationBox={inner->Box(contentAlignment=Alignment.CenterStart){if(draft.isEmpty())Text("发送消息",fontSize=16.sp,color=MaterialTheme.colorScheme.onSurfaceVariant);inner()}})
     CompositionLocalProvider(LocalMinimumInteractiveComponentSize provides 0.dp) {
     FilledIconButton(enabled=!busy&&connected&&!blocked&&thread?.canSend==true&&thread.online&&draft.isNotBlank(),onClick={focus.clearFocus();newest();work{val sent=draft;val result=repo.submit(thread!!,sent);if(result.status in listOf("accepted","dispatching","upstream_queued","codex_accepted")&&draft==sent)draft=""}},modifier=Modifier.size(36.dp).align(Alignment.BottomEnd),colors=IconButtonDefaults.filledIconButtonColors(containerColor=MaterialTheme.colorScheme.onSurface,contentColor=MaterialTheme.colorScheme.surface,disabledContainerColor=MaterialTheme.colorScheme.outlineVariant,disabledContentColor=MaterialTheme.colorScheme.onSurfaceVariant)){Icon(Icons.Outlined.ArrowUpward,"发送",Modifier.size(20.dp))}
     }
    }
   }

  }
 },composerMotion=composerMotion,loading=messages.isEmpty()&&!ready) {
    if(pending!=null&&ConversationPresentation.showPending(pending!!.status))item(key="pending:${pending!!.requestId}"){
     val text=runCatching{JSONObject(pending!!.payload).optString("text")}.getOrDefault("")
     val sentAt=runCatching{JSONObject(pending!!.payload).optLong("created_at")*1000}.getOrDefault(0)
     PendingMessage(pending!!,captureMode,ConversationPresentation.showPendingText(pending!!.status,text,messages.any{it.role=="user"&&it.text==text&&it.ordinal>=sentAt}),text,work,repo)
    }
    items(messages,key={it.id+it.version}){message->MessageCard(repo,message,work)}
    if(messages.isEmpty()&&ready)item{Column(Modifier.fillMaxWidth().padding(vertical=100.dp),horizontalAlignment=Alignment.CenterHorizontally){Text("从这里继续",fontSize=25.sp,fontWeight=FontWeight.SemiBold);Text("电脑保存的消息会显示在此对话中",fontSize=13.sp,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.padding(top=12.dp))}}
    if(historyError)item(key="history_error"){Text("历史加载失败，拉动顶部可重试",fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.fillMaxWidth())}

 }
}

@Composable private fun PendingMessage(pending:Pending,captureMode:Boolean,showText:Boolean,text:String,work:((suspend()->Unit))->Unit,repo:Repository) {
 Column(Modifier.fillMaxWidth(),horizontalAlignment=Alignment.End) {
  if(showText)UserBubble(text)
  Text(ReceiptPresentation.label(pending.status,captureMode),fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.padding(top=8.dp))
  pending.error?.let{Text(ReceiptPresentation.errorLabel(it),fontSize=12.sp,lineHeight=18.sp,color=MaterialTheme.colorScheme.error,modifier=Modifier.padding(top=4.dp))}
  if(pending.status=="accepted")TextButton(onClick={work{repo.cancel(pending)}}){Text("取消发送",fontSize=12.sp)}
  if(pending.status=="submitting")TextButton(onClick={work{repo.retryPending(pending)}}){Text("查询发送结果",fontSize=12.sp)}
 }
}
@Composable private fun UserBubble(text:String) {
 Surface(color=MaterialTheme.colorScheme.surfaceContainerHigh,shape=RoundedCornerShape(24.dp),modifier=Modifier.widthIn(max=320.dp)) {
  SelectionContainer{Text(text,color=MaterialTheme.colorScheme.onSurface,fontSize=16.sp,lineHeight=25.sp,modifier=Modifier.padding(horizontal=18.dp,vertical=12.dp))}
 }
}
@Composable private fun MessageCard(repo:Repository,message:MessageRow,work:((suspend()->Unit))->Unit) {
 var offset by remember(message.id,message.version){mutableIntStateOf(0)};var text by remember(message.id,message.version){mutableStateOf(message.text)}
 Column(Modifier.fillMaxWidth(),horizontalAlignment=if(message.role=="user")Alignment.End else Alignment.Start) {
  if(message.role=="user")UserMessageWithImages(repo,message,text){UserBubble(it)}else MessageBody(text)
  if(message.characters>16000) {
   Row(Modifier.fillMaxWidth().padding(top=12.dp),verticalAlignment=Alignment.CenterVertically){TextButton(enabled=offset>0,onClick={work{val next=maxOf(0,offset-16000);text=repo.chunk(message,next);offset=next}}){Text("上一段",fontSize=12.sp)};Text("${offset+1}–${minOf(offset+16000,message.characters)} / ${message.characters}",fontSize=11.sp,color=MaterialTheme.colorScheme.onSurfaceVariant);TextButton(enabled=offset+16000<message.characters,onClick={work{val next=offset+16000;text=repo.chunk(message,next);offset=next}}){Text("下一段",fontSize=12.sp)}}
  }
 }
}
