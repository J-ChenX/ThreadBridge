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
import androidx.compose.animation.*
import androidx.compose.animation.core.*
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.semantics.selected
import kotlin.math.roundToInt
import kotlin.math.abs
import kotlinx.coroutines.flow.first
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.*
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.draw.clip
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
   // Observe navigation in the root composition before entering theme slots.
   val activeDestination=destination.value
   val dark=isSystemInDarkTheme()
   val colors=if(dark)darkColorScheme(primary=Color(0xFFF5F5F5),onPrimary=Color(0xFF171717),background=Color(0xFF212121),surface=Color(0xFF212121),surfaceContainerLow=Color(0xFF2F2F2F),surfaceContainer=Color(0xFF2F2F2F),outline=Color(0xFF666666),surfaceContainerHigh=Color(0xFF383838),onSurface=Color(0xFFF0F0F0),onSurfaceVariant=Color(0xFFB5B5B5),outlineVariant=Color(0xFF404040))
    else lightColorScheme(primary=Color(0xFF171717),onPrimary=Color.White,background=Color.White,surface=Color.White,surfaceContainerLow=Color(0xFFF5F5F5),surfaceContainer=Color(0xFFF5F5F5),outline=Color(0xFFB7B7B7),surfaceContainerHigh=Color(0xFFEEEEEE),onSurface=Color(0xFF171717),onSurfaceVariant=Color(0xFF686868),outlineVariant=Color(0xFFE8E8E8))
   SideEffect { WindowCompat.getInsetsController(window,window.decorView).apply { isAppearanceLightStatusBars=!dark;isAppearanceLightNavigationBars=!dark } }
   MaterialTheme(colorScheme=colors) { BridgeInteractionTheme{App(repo,activeDestination){destination.value=it}} }
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
 val scope=rememberCoroutineScope();val focus=LocalFocusManager.current
 var paired by remember{mutableStateOf(repo.credentials.token().isNotEmpty())}
 // An empty unpaired drawer has overlapping anchors. Recreate its state when
 // authenticated content appears so the first connection lands on the home page.
 val drawer=key(paired){rememberDrawerState(DrawerValue.Closed)}
 val threadFlow=remember(repo){repo.dao.threads()}
 val hostFlow=remember(repo){repo.dao.hosts()}
 val projectFlow=remember(repo){repo.dao.projects()}
 // Read snapshots in this composition so nested Scaffold/Lazy content receives
 // fresh parameters instead of retaining the initial captured list/read state.
 val threads by rememberUpdatedState(threadFlow.collectAsStateWithLifecycle(initialValue=emptyList()).value)
 val hosts by rememberUpdatedState(hostFlow.collectAsStateWithLifecycle(initialValue=emptyList()).value)
 val creationProjects by rememberUpdatedState(repo.creationProjects.collectAsStateWithLifecycle().value)
 val projects by rememberUpdatedState(projectFlow.collectAsStateWithLifecycle(initialValue=emptyList()).value)
 val epoch by rememberUpdatedState(repo.collection.collectAsStateWithLifecycle().value)
 val unread by rememberUpdatedState(repo.readState.unread.collectAsStateWithLifecycle().value)
 var searchOpen by androidx.compose.runtime.saveable.rememberSaveable{mutableStateOf(false)}
 var query by androidx.compose.runtime.saveable.rememberSaveable{mutableStateOf("")}
 val search by rememberUpdatedState(repo.searchState.collectAsStateWithLifecycle().value)
 LaunchedEffect(query,paired){if(paired){delay(250);repo.searchTitles(query)}}
 val visibleThreads by rememberUpdatedState(if(query.isBlank())threads else (search.rows.filter{titleMatches(it.title,query)}+threads.filter{titleMatches(it.title,query)}).distinctBy{it.id})
 val marks by rememberUpdatedState(repo.library.marks.collectAsStateWithLifecycle().value)
 val redirects by rememberUpdatedState(repo.redirects.collectAsStateWithLifecycle().value)
 LaunchedEffect(redirects,destination){redirects[destination]?.let{navigate(it)}}
 var newConversation by remember{mutableStateOf(false)}
 var favorites by remember{mutableStateOf(false)}
 LaunchedEffect(threads){repo.readState.restore(threads)}
 var collapsed by androidx.compose.runtime.saveable.rememberSaveable{mutableStateOf(listOf<String>())}
 var expandedProjects by androidx.compose.runtime.saveable.rememberSaveable{mutableStateOf(listOf<String>())}
 fun toggleExpanded(id:String){expandedProjects=if(id in expandedProjects)expandedProjects-id else expandedProjects+id}
 var searchCollapsed by androidx.compose.runtime.saveable.rememberSaveable{mutableStateOf(listOf<String>())}
 LaunchedEffect(query){searchCollapsed=emptyList()}
 val activeCollapsed by rememberUpdatedState(if(query.isBlank())collapsed else searchCollapsed)
 var deleteTarget by remember{mutableStateOf<ThreadRow?>(null)}
 var confirmDelete by remember{mutableStateOf<ThreadRow?>(null)}
 var groupTarget by remember{mutableStateOf<ConversationScope?>(null)}
 var selecting by androidx.compose.runtime.saveable.rememberSaveable{mutableStateOf(false)}
 var selectedIds by androidx.compose.runtime.saveable.rememberSaveable{mutableStateOf(listOf<String>())}
 var confirmBatch by remember{mutableStateOf<Set<String>?>(null)}
 val snackbar=remember{SnackbarHostState()}
 fun notify(text:String){scope.launch{snackbar.showSnackbar(text)}}
 fun toggleSelection(id:String){selectedIds=if(id in selectedIds)selectedIds-id else selectedIds+id}
 fun finishSelection(){selecting=false;selectedIds=emptyList();confirmBatch=null}
 fun startSelection(id:String?=null){selecting=true;selectedIds=listOfNotNull(id);deleteTarget=null;focus.clearFocus();navigate(null);scope.launch{drawer.close()}}

 val connection by rememberUpdatedState(repo.connection.collectAsStateWithLifecycle().value);val connectionIssue by rememberUpdatedState(repo.connectionIssue.collectAsStateWithLifecycle().value);val warning by rememberUpdatedState(repo.captureWarning.collectAsStateWithLifecycle().value)
 var error by remember{mutableStateOf<String?>(null)};var busy by remember{mutableStateOf(false)};var refreshing by remember{mutableStateOf(false)}
 var busyIndicator by remember{mutableStateOf(false)}
 LaunchedEffect(busy){if(busy){delay(180);busyIndicator=true}else busyIndicator=false}
 var settings by remember{mutableStateOf(false)};var details by remember{mutableStateOf(false)};var menu by remember{mutableStateOf(false)}
 fun work(block:suspend()->Unit){scope.launch{busy=true;try{block()}catch(e:CancellationException){throw e}catch(e:Exception){error=e.message?:"操作失败，请稍后重试"}finally{busy=false}}}
 fun refresh(){if(refreshing)return;refreshing=true;scope.launch{try{repo.sync()}catch(e:CancellationException){throw e}catch(e:Exception){error=e.message?:"同步失败，请稍后重试"}finally{refreshing=false}}}
 fun open(id:String?){threads.find{it.id==id}?.let{repo.readState.markRead(it)};focus.clearFocus();navigate(id);scope.launch{drawer.close()}}
 LaunchedEffect(destination,paired){repo.selected=destination;if(paired)runCatching{repo.sync()}}
 BackHandler(paired&&(drawer.isOpen||destination!=null||selecting)){if(selecting){if(!busy)finishSelection()}else if(drawer.isOpen)scope.launch{drawer.close()}else navigate(null)}
 LaunchedEffect(epoch){finishSelection();groupTarget=null}
 LaunchedEffect(visibleThreads,selecting){if(selecting)selectedIds=selectedIds.filter{selected->visibleThreads.any{it.id==selected}}}
 LaunchedEffect(epoch){if(epoch.isNotEmpty()&&destination!=null&&repo.selected==null)navigate(null)}
 fun openResult(id:String){if(selecting){if(!busy)toggleSelection(id);return};val row=visibleThreads.find{it.id==id};if(threads.none{it.id==id}&&row!=null)work{repo.restoreSearchedThread(row);open(id)}else open(id)}
 val current by rememberUpdatedState(threads.find{it.id==destination})
 fun toggle(id:String){if(query.isBlank())collapsed=if(id in collapsed)collapsed-id else collapsed+id else searchCollapsed=if(id in searchCollapsed)searchCollapsed-id else searchCollapsed+id}
 ModalNavigationDrawer(drawerState=drawer,gesturesEnabled=paired,drawerContent={
  if(paired)ModalDrawerSheet(drawerContainerColor=MaterialTheme.colorScheme.surfaceContainerLow,modifier=Modifier.width(300.dp)) {
   Row(Modifier.fillMaxWidth().heightIn(min=48.dp).padding(horizontal=10.dp),verticalAlignment=Alignment.CenterVertically){
    TextButton(onClick={open(null)}){Icon(BridgeIcons.Home,null,Modifier.size(18.dp));Spacer(Modifier.width(8.dp));Text("主页")}
    Spacer(Modifier.weight(1f));BridgeIconButton(active=searchOpen,onClick={searchOpen=!searchOpen;if(!searchOpen)query=""}){Icon(BridgeIcons.Search,"搜索对话标题")};BridgeIconButton(onClick={scope.launch{drawer.close()};focus.clearFocus();newConversation=true}){Icon(BridgeIcons.Add,"新建对话")}
    BridgeIconButton(onClick={scope.launch{drawer.close()}}){Icon(BridgeIcons.Back,"收起对话列表")}
   }
   AnimatedVisibility(searchOpen,enter=expandVertically(expandFrom=Alignment.Top,animationSpec=tween(BridgeMotion.Reveal))+fadeIn(tween(BridgeMotion.Fade)),exit=shrinkVertically(shrinkTowards=Alignment.Top,animationSpec=tween(BridgeMotion.Icon))+fadeOut(tween(BridgeMotion.Press))){ConversationSearchField(query,{query=it},search.loading,search.offline)}
   val drawerList=rememberLazyListState();val deviceHeights=remember{mutableStateMapOf<String,Int>()};val deviceHeight=with(androidx.compose.ui.platform.LocalDensity.current){48.dp.roundToPx()}
   LazyColumn(Modifier.weight(1f),state=drawerList,contentPadding=PaddingValues(start=6.dp,end=6.dp,bottom=8.dp)){
    deviceItems(hosts,visibleThreads,activeCollapsed,destination,::toggle,::openResult,{deleteTarget=it},unread,marks,true,expanded=expandedProjects.toSet(),toggleExpanded=::toggleExpanded,searching=query.isNotBlank(),pinnedProject={pinnedProjectKey(drawerList,it,visibleThreads,deviceHeights[it]?:deviceHeight)},onDeviceHeight={host,height->deviceHeights[host]=height},groupActions={groupTarget=it},selecting=selecting,selectedIds=selectedIds.toSet())
   }
  }
 }) {
  val navigationButton: @Composable ()->Unit={ConversationIconButton(floating=destination!=null,onClick={focus.clearFocus();scope.launch{drawer.open()}}){Icon(BridgeIcons.Menu,"打开对话列表")}}
  val moreButton: @Composable ()->Unit={Box{ConversationIconButton(floating=false,onClick={focus.clearFocus();menu=true}){Icon(BridgeIcons.More,"更多选项")};DropdownMenu(menu,{menu=false},modifier=Modifier.width(224.dp),shape=RoundedCornerShape(20.dp),containerColor=MaterialTheme.colorScheme.surfaceContainerLow,tonalElevation=0.dp,shadowElevation=6.dp){
    BridgeMenuItem("多选会话",BridgeIcons.Check){menu=false;startSelection()}
    BridgeMenuItem("收藏对话",BridgeIcons.Star){menu=false;favorites=true}
    BridgeMenuItem("连接状态",BridgeIcons.Info){menu=false;details=true}
    BridgeMenuItem("设置",BridgeIcons.Settings){menu=false;settings=true}
   }}}
  Scaffold(containerColor=MaterialTheme.colorScheme.background,snackbarHost={SnackbarHost(snackbar)},
   contentWindowInsets=if(paired&&destination!=null)WindowInsets(0,0,0,0)else ScaffoldDefaults.contentWindowInsets,
   topBar={if(paired&&destination==null&&selecting)TopAppBar(
    expandedHeight=48.dp,title={Text("已选 ${selectedIds.size} 个",fontSize=18.sp)},
    navigationIcon={BridgeIconButton(enabled=!busy,onClick={finishSelection()}){Icon(BridgeIcons.Close,"取消多选")}},
    actions={val all=visibleThreads.isNotEmpty()&&visibleThreads.all{it.id in selectedIds};TextButton(enabled=!busy&&visibleThreads.isNotEmpty(),onClick={selectedIds=if(all)emptyList()else visibleThreads.map{it.id}}){Text(if(all)"取消全选"else"全选")};BridgeIconButton(enabled=!busy&&selectedIds.isNotEmpty(),onClick={confirmBatch=selectedIds.toSet()}){Icon(BridgeIcons.Delete,"删除选中的会话",tint=MaterialTheme.colorScheme.error)}},
    colors=TopAppBarDefaults.topAppBarColors(containerColor=MaterialTheme.colorScheme.background))else if(paired&&destination==null)TopAppBar(
    expandedHeight=48.dp,
    title={Text("设备列表",fontSize=18.sp,fontWeight=FontWeight.SemiBold)},
    navigationIcon=navigationButton,actions={BridgeIconButton(active=searchOpen,onClick={searchOpen=!searchOpen;if(!searchOpen)query=""}){Icon(BridgeIcons.Search,"搜索对话标题")};BridgeIconButton(onClick={focus.clearFocus();newConversation=true}){Icon(BridgeIcons.Add,"新建对话")};moreButton()},
    colors=TopAppBarDefaults.topAppBarColors(containerColor=MaterialTheme.colorScheme.background))}) { padding->
   val liveThreads by threadFlow.collectAsStateWithLifecycle(initialValue=emptyList())
   val liveHosts by hostFlow.collectAsStateWithLifecycle(initialValue=emptyList())
   val liveUnread by repo.readState.unread.collectAsStateWithLifecycle()
   val liveSearch by repo.searchState.collectAsStateWithLifecycle()
   val liveMarks by repo.library.marks.collectAsStateWithLifecycle()
   val liveVisible=if(query.isBlank())liveThreads else (liveSearch.rows.filter{titleMatches(it.title,query)}+liveThreads.filter{titleMatches(it.title,query)}).distinctBy{it.id}
   Box(Modifier.fillMaxSize().padding(padding).consumeWindowInsets(padding)) {
    if(!paired)PairScreen(busy,error){server,code,lan,direct->work{if(direct)repo.configureConnection(server,code,lan)else repo.pair(server,code,lan);paired=true;error=null}}
    else Column(Modifier.fillMaxSize()) {
     if(destination==null&&warning!=null)Row(Modifier.fillMaxWidth().softClickable{details=true}.heightIn(min=48.dp).padding(horizontal=20.dp,vertical=8.dp),verticalAlignment=Alignment.CenterVertically){Icon(BridgeIcons.Info,null,tint=MaterialTheme.colorScheme.error,modifier=Modifier.size(14.dp));Spacer(Modifier.width(7.dp));Text(CapturePresentation.summary(warning!!),fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant);Spacer(Modifier.weight(1f));Text("查看",fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)}
     if(destination==null)AnimatedVisibility(searchOpen,enter=expandVertically(expandFrom=Alignment.Top,animationSpec=tween(BridgeMotion.Reveal))+fadeIn(tween(BridgeMotion.Fade)),exit=shrinkVertically(shrinkTowards=Alignment.Top,animationSpec=tween(BridgeMotion.Icon))+fadeOut(tween(BridgeMotion.Press))){ConversationSearchField(query,{query=it},search.loading,search.offline)}
     if(destination==null&&search.more)Text("仅显示前 1000 个匹配结果，请缩小关键词范围",fontSize=12.sp,modifier=Modifier.padding(horizontal=20.dp))
     if(destination==null&&selecting)Text("点击选择会话；全选包含折叠中的对话",fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.padding(horizontal=20.dp,vertical=8.dp))
     if(destination==null)ConversationList(liveHosts,liveVisible,activeCollapsed,::toggle,::openResult,{deleteTarget=it},liveUnread,liveMarks,refreshing,::refresh,query.isNotBlank(),expandedProjects.toSet(),::toggleExpanded,groupActions={groupTarget=it},selecting=selecting,selectedIds=selectedIds.toSet())
     else key(destination,epoch){ChatScreen(repo,destination,liveThreads.find{it.id==destination},busy,refreshing,::refresh,header={ConversationHeader(navigationButton,liveThreads.find{it.id==destination})},work={work(it)})}
    }
    if(busyIndicator)LinearProgressIndicator(Modifier.fillMaxWidth().height(2.dp).align(Alignment.TopCenter),color=MaterialTheme.colorScheme.onSurface)
   }
  }
 }
 if(deleteTarget!=null)ModalBottomSheet(onDismissRequest={deleteTarget=null},containerColor=MaterialTheme.colorScheme.surface){
  Text(displayTitle(deleteTarget!!,hosts),fontSize=16.sp,fontWeight=FontWeight.SemiBold,maxLines=2,overflow=TextOverflow.Ellipsis,modifier=Modifier.padding(horizontal=24.dp,vertical=12.dp))
  SheetAction(if(deleteTarget!!.id in marks.pinned)"取消置顶"else"置顶对话",if(deleteTarget!!.id in marks.pinned)BridgeIcons.PinFilled else BridgeIcons.Pin){repo.library.pin(deleteTarget!!.id);deleteTarget=null}
  SheetAction(if(deleteTarget!!.id in marks.favorites)"取消收藏"else"收藏对话",if(deleteTarget!!.id in marks.favorites)BridgeIcons.StarFilled else BridgeIcons.Star){repo.library.favorite(deleteTarget!!.id);deleteTarget=null}
  SheetAction("多选会话",BridgeIcons.Check){startSelection(deleteTarget!!.id)}
  SheetAction("删除对话",BridgeIcons.Delete,danger=true){confirmDelete=deleteTarget;deleteTarget=null}
  Spacer(Modifier.height(12.dp))
 }
 if(groupTarget!=null)ModalBottomSheet(onDismissRequest={groupTarget=null},containerColor=MaterialTheme.colorScheme.surface){
  val target=groupTarget!!
  val group=threads.filter{target.contains(it)}
  val count=group.count{it.id in unread}
  Text(target.title,fontSize=16.sp,fontWeight=FontWeight.SemiBold,maxLines=2,overflow=TextOverflow.Ellipsis,modifier=Modifier.padding(horizontal=24.dp,vertical=12.dp))
  SheetAction("全部标为已读 · $count 个未读",BridgeIcons.Check){repo.readState.markRead(group);groupTarget=null;notify(if(count==0)"没有未读通知"else"已将 $count 个会话标为已读")}
  Spacer(Modifier.height(12.dp))
 }
 if(confirmBatch!=null)AlertDialog(shape=BridgeShapes.Panel,onDismissRequest={if(!busy)confirmBatch=null},title={Text("删除 ${confirmBatch!!.size} 个会话？")},text={Text("将删除手机服务中的同步副本，并停止同步这些会话。电脑 Codex 中的原始对话会保留。")},confirmButton={TextButton(enabled=!busy,onClick={val targets=confirmBatch!!;confirmBatch=null;work{repo.deleteThreads(targets){deleted->selectedIds=selectedIds-deleted};finishSelection();notify("已删除 ${targets.size} 个会话")}}){Text("删除",color=MaterialTheme.colorScheme.error)}},dismissButton={TextButton(enabled=!busy,onClick={confirmBatch=null}){Text("取消")}})
 if(newConversation)NewConversationDialog(hosts,threads,projects,creationProjects,busy,{newConversation=false}){host,project->work{val id=repo.createBlank(host,project);newConversation=false;open(id)}}
 if(favorites)AlertDialog(shape=BridgeShapes.Panel,onDismissRequest={favorites=false},title={Text("收藏对话")},text={
  val saved=threads.filter{it.id in marks.favorites}.sortedWith(compareByDescending<ThreadRow>{conversationActivity(it)}.thenBy{it.id})
  if(saved.isEmpty())Text("长按对话可加入收藏")else LazyColumn(Modifier.heightIn(max=420.dp)){items(saved,key={it.id}){thread->Column(Modifier.fillMaxWidth().softCombinedClickable(onClick={favorites=false;open(thread.id)},onLongClick={favorites=false;deleteTarget=thread}).padding(vertical=12.dp)){Text(displayTitle(thread,hosts),maxLines=2,overflow=TextOverflow.Ellipsis);Text("${hosts.find{it.id==thread.groupHostId}?.let(::hostLabel)?:thread.groupHostId} · ${threadProjectLabel(thread)}",fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)}}}
 },confirmButton={TextButton(onClick={favorites=false}){Text("关闭")}})
 if(confirmDelete!=null)AlertDialog(shape=BridgeShapes.Panel,onDismissRequest={confirmDelete=null},title={Text("删除这个对话？")},text={Text("将删除手机服务中的同步副本，并停止同步此对话。电脑 Codex 中的原始对话会保留。")},confirmButton={TextButton(onClick={val target=confirmDelete!!;work{repo.deleteThread(target.id);if(destination==target.id)open(null);confirmDelete=null}}){Text("删除",color=MaterialTheme.colorScheme.error)}},dismissButton={TextButton(onClick={confirmDelete=null}){Text("取消")}})
 if(error!=null&&paired)AlertDialog(shape=BridgeShapes.Panel,onDismissRequest={error=null},title={Text("暂时无法完成")},text={Text(error!!)},confirmButton={TextButton(onClick={error=null}){Text("知道了")}})
 if(details)AlertDialog(shape=BridgeShapes.Panel,onDismissRequest={details=false},title={Text("连接状态")},text={Column(verticalArrangement=Arrangement.spacedBy(14.dp)){Text(connection);connectionIssue?.let{Text(it,fontSize=13.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)};Text("服务器：${repo.credentials.server}",fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant);current?.let{Text(stateLabel(it.status));if(it.status in listOf("queue_ready","capture_only","resume_ready"))Text("显示已同步的用户文字、图片、过程说明与最终回复；尚未同步的历史请在电脑查看。",fontSize=13.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)};if(warning!=null)Text(warning!!);Text("已配对电脑：${hosts.size} 台\n已保存对话：${threads.size} 个",fontSize=13.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)}},confirmButton={TextButton(onClick={details=false}){Text("关闭")}})
 if(settings)ConnectionSettingsDialog(repo,{settings=false}){settings=false;work{repo.forget();paired=false;navigate(null)}}
}
private fun displayTitle(thread:ThreadRow,hosts:List<HostRow>):String {
 val name=hosts.find{it.id==thread.hostId}?.name?:return thread.title
 return thread.title.removePrefix("$name · ").removePrefix("windows · ")
}
private fun hostLabel(host:HostRow)=if(host.name.equals("windows",true))"Windows"else host.name
@OptIn(ExperimentalFoundationApi::class)
private fun LazyListScope.deviceItems(hosts:List<HostRow>,threads:List<ThreadRow>,collapsed:List<String>,selected:String?,toggle:(String)->Unit,open:(String)->Unit,actions:(ThreadRow)->Unit,unread:Set<String>,marks:ConversationMarks,inDrawer:Boolean,isPinned:(String)->Boolean={false},searching:Boolean=false,pinnedProject:(String)->String?={null},onDeviceHeight:(String,Int)->Unit={_,_->},expanded:Set<String> = emptySet(),toggleExpanded:(String)->Unit={},groupActions:(ConversationScope)->Unit={},selecting:Boolean=false,selectedIds:Set<String> = emptySet()){
 for((position,device) in deviceGroups(hosts,threads,unread,marks.pinned).map{if(searching)it.copy(preview=it.threads)else it}.filter{!searching||it.threads.isNotEmpty()}.withIndex()){
  if(position>0)item(key="device_gap:${device.host.id}"){Spacer(Modifier.height(6.dp))}
  val host=device.host
  val projects=projectGroups(device.threads,marks.pinned)
  stickyHeader(key="device:${host.id}"){
   val background=if(inDrawer)MaterialTheme.colorScheme.surfaceContainerLow else MaterialTheme.colorScheme.background
   Box(Modifier.fillMaxWidth().drawWithCache{
    val fadeHeight=16.dp.toPx();val fade=Brush.verticalGradient(0f to background,1f to background.copy(alpha=0f),startY=size.height,endY=size.height+fadeHeight)
    onDrawWithContent{drawContent();if(isPinned(host.id))drawRect(fade,topLeft=Offset(0f,size.height),size=Size(size.width,fadeHeight))}
   }){Surface(color=background){Column{
    Row(Modifier.fillMaxWidth().softCombinedClickable(onClick={toggle(host.id)},onLongClick={if(!selecting)groupActions(ConversationScope(host.id,title=hostLabel(host)))}).onSizeChanged{onDeviceHeight(host.id,it.height)}.heightIn(min=48.dp).padding(horizontal=10.dp,vertical=8.dp),verticalAlignment=Alignment.CenterVertically){
     Icon(BridgeIcons.Computer,null,Modifier.size(20.dp),tint=MaterialTheme.colorScheme.onSurfaceVariant);Spacer(Modifier.width(10.dp))
     Text(hostLabel(host),fontSize=15.sp,lineHeight=20.sp,fontWeight=FontWeight.SemiBold,modifier=Modifier.weight(1f))
     if(!host.online)Icon(BridgeIcons.Offline,"设备离线",Modifier.padding(end=8.dp).size(18.dp),tint=MaterialTheme.colorScheme.onSurfaceVariant)
     if(device.unread)UnreadDot("未读设备 ${host.name}")
     Spacer(Modifier.width(8.dp));Text("${device.threads.size}",fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)
     BridgeChevron(host.id !in collapsed,if(host.id in collapsed)"展开设备 ${host.name}"else"折叠设备 ${host.name}",Modifier.padding(start=4.dp))
    }
    if(host.id !in collapsed)projects.find{projectKey(host.id,it.project,it.known)==pinnedProject(host.id)}?.let{project->ProjectHeader(host.id,project,collapsed,toggle,unread,true,if(selecting||searching||projectKey(host.id,project.project,project.known) in expanded)project.threads.size else projectPreview(project,unread,marks.pinned).size,groupActions=groupActions,selecting=selecting)}
   }}}
  }
  if(host.id !in collapsed){
   if(device.preview.isEmpty())item(key="empty:${host.id}"){Text("暂无新对话",fontSize=13.sp,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.padding(start=38.dp,top=4.dp,bottom=8.dp))}
   for(project in projects){
    val projectId=projectKey(host.id,project.project,project.known)
    val preview=projectPreview(project,unread,marks.pinned)
    val shown=if(selecting||searching||projectId in expanded)project.threads else preview
    item(key=projectId,contentType="project"){ProjectHeader(host.id,project,collapsed,toggle,unread,false,shown.size,groupActions=groupActions,selecting=selecting)}
    if(projectId !in collapsed){items(shown,key={it.id},contentType={"conversation"}){thread->
     Surface(color=if(thread.id==selected||thread.id in selectedIds)MaterialTheme.colorScheme.surfaceContainerHigh else Color.Transparent,shape=BridgeShapes.Row,modifier=Modifier.fillMaxWidth().softCombinedClickable(onClick={open(thread.id)},onLongClick={if(selecting)open(thread.id)else actions(thread)}).semantics{if(selecting)this.selected=thread.id in selectedIds}){
      Row(Modifier.heightIn(min=48.dp).padding(start=48.dp,end=10.dp,top=6.dp,bottom=6.dp),verticalAlignment=Alignment.CenterVertically){
       if(selecting){Checkbox(checked=thread.id in selectedIds,onCheckedChange=null,modifier=Modifier.padding(end=8.dp).size(24.dp))}
       Text(displayTitle(thread,hosts),maxLines=2,overflow=TextOverflow.Ellipsis,fontSize=14.sp,lineHeight=19.sp,fontWeight=if(thread.id in unread)FontWeight.Medium else FontWeight.Normal,modifier=Modifier.weight(1f))
       ConversationTaskStatus(thread,Modifier.padding(start=8.dp))
       if(thread.id in marks.pinned)Icon(BridgeIcons.PinFilled,"已置顶",Modifier.padding(start=6.dp).size(14.dp),tint=MaterialTheme.colorScheme.onSurfaceVariant)
       if(thread.id in marks.favorites)Icon(BridgeIcons.StarFilled,"已收藏",Modifier.padding(start=6.dp).size(14.dp),tint=MaterialTheme.colorScheme.onSurfaceVariant)
       if(thread.id in unread){Spacer(Modifier.width(8.dp));UnreadDot("未读对话 ${thread.title}")}
      }
     }
    }
    if(!selecting&&!searching&&preview.size<project.threads.size)item(key="more:$projectId"){
     val angle by animateFloatAsState(if(projectId in expanded)-90f else 90f,tween(BridgeMotion.Icon),label="project show all")
     Row(Modifier.fillMaxWidth().softClickable{toggleExpanded(projectId)}.heightIn(min=48.dp).padding(start=48.dp,end=10.dp),verticalAlignment=Alignment.CenterVertically){Text(if(projectId in expanded)"收起到最近与未读"else"展开全部 · 另 ${project.threads.size-preview.size} 个对话",fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.weight(1f));Icon(BridgeIcons.Chevron,null,Modifier.padding(start=4.dp).size(18.dp).graphicsLayer{rotationZ=angle},tint=MaterialTheme.colorScheme.onSurfaceVariant)}
    }
    }
   }
  }
 }
}
@Composable private fun ProjectHeader(host:String,project:ProjectGroup,collapsed:List<String>,toggle:(String)->Unit,unread:Set<String>,pinned:Boolean,shown:Int=project.threads.size,groupActions:(ConversationScope)->Unit={},selecting:Boolean=false){
 val id=projectKey(host,project.project,project.known);val label=if(!project.known)"项目待同步"else project.threads.firstOrNull()?.projectName?.takeIf{it.isNotBlank()}?:projectLabel(project.project)
 Row(Modifier.fillMaxWidth().softCombinedClickable(onClick={toggle(id)},onLongClick={if(!selecting)groupActions(ConversationScope(host,project.project,project.known,label))}).heightIn(min=48.dp).padding(start=30.dp,end=10.dp,top=6.dp,bottom=6.dp).semantics{if(pinned)contentDescription="固定项目 $label"},verticalAlignment=Alignment.CenterVertically){
  Icon(BridgeIcons.Folder,null,Modifier.size(18.dp),tint=MaterialTheme.colorScheme.onSurfaceVariant);Spacer(Modifier.width(8.dp));Text(label,fontSize=13.sp,fontWeight=FontWeight.Medium,maxLines=1,overflow=TextOverflow.Ellipsis,modifier=Modifier.weight(1f));if(project.threads.any{it.id in unread})UnreadDot("未读项目 $label");Text(if(shown==project.threads.size)"${project.threads.size}"else"$shown/${project.threads.size}",fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.padding(start=8.dp));BridgeChevron(id !in collapsed,if(id in collapsed)"展开项目 $label"else"折叠项目 $label",Modifier.padding(start=4.dp))
 }
}
private fun pinnedProjectKey(list:androidx.compose.foundation.lazy.LazyListState,host:String,threads:List<ThreadRow>,deviceHeight:Int):String? {
 if(!list.canScrollBackward)return null
 val item=list.layoutInfo.visibleItemsInfo.sortedBy{it.index}.firstOrNull{it.key.toString().let{k->!k.startsWith("device:")&&!k.startsWith("device_gap:")&&!k.startsWith("empty:")}&&it.offset+it.size>deviceHeight}?:return null
 val row=threads.find{it.id==item.key}
 if(row!=null)return if(row.groupHostId==host)projectKey(host,row.project,row.projectKnown)else null
 val key=item.key.toString().removePrefix("more:");return key.takeIf{it.startsWith("project:$host:")&&item.offset<deviceHeight}
}
@Composable private fun UnreadDot(description:String){Box(Modifier.size(6.dp).background(Color(0xFF2383E2),CircleShape).semantics{contentDescription=description})}
@OptIn(ExperimentalMaterial3Api::class)
@Composable private fun ConversationList(hosts:List<HostRow>,threads:List<ThreadRow>,collapsed:List<String>,toggle:(String)->Unit,open:(String)->Unit,delete:(ThreadRow)->Unit,unread:Set<String>,marks:ConversationMarks,refreshing:Boolean,refresh:()->Unit,searching:Boolean=false,expanded:Set<String> = emptySet(),toggleExpanded:(String)->Unit={},groupActions:(ConversationScope)->Unit={},selecting:Boolean=false,selectedIds:Set<String> = emptySet()){
 val list=rememberLazyListState()
 val deviceHeights=remember{mutableStateMapOf<String,Int>()}
 val deviceHeight=with(androidx.compose.ui.platform.LocalDensity.current){48.dp.roundToPx()}
 var keepAtStart by remember{mutableStateOf(true)}
 LaunchedEffect(list){snapshotFlow{list.isScrollInProgress}.collect{scrolling->keepAtStart=if(scrolling)false else !list.canScrollBackward}}
 // Key anchoring can otherwise retain the former first device after a newer
 // group moves above it. Keep an idle overview at the beginning; preserve
 // the anchor only when the user has actually scrolled into the list.
 LaunchedEffect(threads){if(keepAtStart)list.requestScrollToItem(0)}
 PullToRefreshBox(isRefreshing=refreshing,onRefresh={keepAtStart=true;list.requestScrollToItem(0);refresh()},modifier=Modifier.fillMaxSize()){
 LazyColumn(Modifier.fillMaxSize(),state=list,contentPadding=PaddingValues(start=8.dp,end=8.dp,top=0.dp,bottom=8.dp)){
  if(threads.isEmpty())item{Text(if(searching)"没有匹配的对话"else"点击右上角 ＋ 新建对话",fontSize=13.sp,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.padding(horizontal=12.dp,vertical=8.dp))}
  deviceItems(hosts,threads,collapsed,null,toggle,open,delete,unread,marks,false,isPinned={hostId->list.canScrollBackward&&list.layoutInfo.visibleItemsInfo.any{it.key=="device:$hostId"&&it.offset<=0}},searching=searching,pinnedProject={pinnedProjectKey(list,it,threads,deviceHeights[it]?:deviceHeight)},onDeviceHeight={host,height->deviceHeights[host]=height},expanded=expanded,toggleExpanded=toggleExpanded,groupActions=groupActions,selecting=selecting,selectedIds=selectedIds)
 }
}
}
@Composable private fun PairScreen(busy:Boolean,error:String?,pair:(String,String,Boolean,Boolean)->Unit) {
 var server by rememberSaveableCompat(BuildConfig.PUBLIC_TEST_SERVER);var code by remember{mutableStateOf("")};var direct by remember{mutableStateOf(false)};var showKey by remember{mutableStateOf(false)};var lan by remember{mutableStateOf(false)};var scanError by remember{mutableStateOf<String?>(null)}
 val scanner=rememberLauncherForActivityResult(ScanContract()){result->if(result.contents!=null)try{val value=JSONObject(result.contents);server=value.getString("server");code=value.getString("code");direct=false;lan=server.startsWith("http://")}catch(_:Exception){scanError="这不是续桥配对二维码"}}
 LazyColumn(Modifier.fillMaxSize().imePadding(),contentPadding=PaddingValues(horizontal=28.dp,vertical=50.dp),verticalArrangement=Arrangement.spacedBy(20.dp)) {
  item{Icon(BridgeIcons.Chat,null,Modifier.size(38.dp));Text("连接你的电脑",fontSize=30.sp,fontWeight=FontWeight.SemiBold,modifier=Modifier.padding(top=28.dp));Text(if(direct)"输入地址与连接密钥，让对话继续。"else"输入配对码，让对话继续。",fontSize=15.sp,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.padding(top=12.dp))}
  item{Row(horizontalArrangement=Arrangement.spacedBy(8.dp)){FilterChip(selected=!direct,enabled=!busy,onClick={direct=false;code="";showKey=false},label={Text("配对码")});FilterChip(selected=direct,enabled=!busy,onClick={direct=true;code="";showKey=false},label={Text("连接密钥")})}}
  item{OutlinedTextField(server,{server=it},enabled=!busy,label={Text("服务器地址")},singleLine=true,modifier=Modifier.fillMaxWidth(),shape=RoundedCornerShape(16.dp))}
  item{if(direct)ConnectionKeyField(code,{code=it},showKey,{showKey=!showKey},busy,false)else OutlinedTextField(code,{code=it},enabled=!busy,label={Text("一次性配对码")},minLines=2,maxLines=3,modifier=Modifier.fillMaxWidth(),shape=RoundedCornerShape(16.dp))}
  item{Row(verticalAlignment=Alignment.CenterVertically){Switch(lan,{lan=it},enabled=!busy);Spacer(Modifier.width(12.dp));Text("局域网测试",fontSize=14.sp)}}
  item{Button(onClick={pair(server,code,lan,direct)},enabled=!busy&&server.isNotBlank()&&code.isNotBlank(),modifier=Modifier.fillMaxWidth().height(52.dp),shape=RoundedCornerShape(26.dp)){Text(if(busy)"正在连接…"else"连接",fontSize=16.sp)}}
  if(!direct)item{OutlinedButton(enabled=!busy,onClick={scanner.launch(ScanOptions().setDesiredBarcodeFormats(ScanOptions.QR_CODE).setPrompt("扫描电脑上的配对码").setBeepEnabled(false))},modifier=Modifier.fillMaxWidth().height(52.dp),shape=RoundedCornerShape(26.dp)){Icon(BridgeIcons.Scan,null);Spacer(Modifier.width(10.dp));Text("扫描二维码")}}
  if(error!=null||scanError!=null)item{Text(error?:scanError!!,color=MaterialTheme.colorScheme.error,fontSize=13.sp)}
  item{Text(if(direct)"使用 Hub 的手机访问密钥，非 cpolar 令牌。\n密钥加密保存在你的手机上。"else"配对码 5 分钟后过期，只能使用一次。\n凭证保存在你的手机上。",fontSize=12.sp,lineHeight=20.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)}
 }
}
// Keep the same multiline IME configuration when focus changes. Flatten only
// the collapsed preview, preserving every draft character and cursor offset.
private val CompactDraftTransformation=VisualTransformation { text->TransformedText(AnnotatedString(text.text.replace('\n',' ')),OffsetMapping.Identity) }
@Composable private fun rememberSaveableCompat(initial:String)=androidx.compose.runtime.saveable.rememberSaveable{mutableStateOf(initial)}
@OptIn(ExperimentalLayoutApi::class)
@Composable private fun ChatScreen(repo:Repository,id:String,thread:ThreadRow?,busy:Boolean,refreshing:Boolean,refresh:()->Unit,header:@Composable ()->Unit,work:((suspend()->Unit))->Unit) {
 val currentThread by rememberUpdatedState(thread)
 val currentBusy by rememberUpdatedState(busy)
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
 val connection by repo.connection.collectAsStateWithLifecycle();val connected by remember{derivedStateOf{connection.startsWith("已连接")}}
 val blocked by remember{derivedStateOf{pending?.status in listOf("submitting","accepted","dispatching","upstream_queued","unknown")}}
 val captureMode=currentThread?.status in listOf("queue_ready","capture_only","resume_ready")
 val atLatest by remember{derivedStateOf{before==Long.MAX_VALUE&&list.firstVisibleItemIndex==0&&list.firstVisibleItemScrollOffset==0}}
 LaunchedEffect(currentThread?.updated,currentThread?.revision,currentThread?.messageRevision,atLatest,ready){if(ready&&atLatest)currentThread?.let{repo.readState.markRead(it)}}
 fun older(){val cursor=nextPage?:return;if(loadingHistory)return;scope.launch{loadingHistory=true;historyError=false;try{nextPage=repo.loadMessages(id,cursor.ordinal,cursor.id);if(visibleLimit>=5000){before=cursor.ordinal;beforeId=cursor.id;visibleLimit=100}else visibleLimit=minOf(5000,visibleLimit+30)}catch(e:CancellationException){throw e}catch(_:Exception){historyError=true}finally{loadingHistory=false}}}
 // Reposition during the next measure without waiting for a network round trip
 // or animating through every paragraph in a long response.
 fun newest(){val changedWindow=before!=Long.MAX_VALUE;if(changedWindow)visibleLimit=100;before=Long.MAX_VALUE;beforeId="";list.requestScrollToItem(0);if(changedWindow)scope.launch{runCatching{nextPage=repo.loadMessages(id)}}}
 fun loadOlder(){
  if(loadingHistory||currentBusy||blocked)return
  if(nextPage!=null)older()
  else if(currentThread?.historyCursor!=null)work{repo.submit(currentThread!!,"",true)}
  else if(historyError)scope.launch{loadingHistory=true;try{nextPage=repo.loadMessages(id);historyError=false}catch(e:CancellationException){throw e}catch(_:Exception){historyError=true}finally{loadingHistory=false}}
 }
 LaunchedEffect(id){draft=repo.draft(id);draftReady=true;try{nextPage=repo.loadMessages(id)}catch(e:CancellationException){throw e}catch(_:Exception){historyError=true}finally{ready=true}}
 LaunchedEffect(draftReady,pending?.requestId,pending?.status,currentBusy){
  val receipt=pending
  if(draftReady&&!currentBusy&&receipt!=null&&composerSubmitted(receipt.status)){
   val payload=runCatching{JSONObject(receipt.payload)}.getOrNull()
   val sent=payload?.optString("text").orEmpty()
   if(payload?.optString("kind")!="history"&&sent.isNotEmpty()&&draft==sent){draft="";repo.saveDraft(id,"")}
  }
 }
 LaunchedEffect(pending?.status){if(pending?.status=="history_loaded"){
  // The computer has inserted records older than our cached range. Fetch
  // from that range's boundary while preserving the current reading position.
  val oldest=messages.lastOrNull();loadingHistory=true
  try{nextPage=repo.loadMessages(id,oldest?.ordinal,oldest?.id);visibleLimit=minOf(5000,visibleLimit+30);historyError=false}
  catch(e:CancellationException){throw e}catch(_:Exception){historyError=true}finally{loadingHistory=false}
 }}
 LaunchedEffect(messages.firstOrNull()?.id){if(ready&&atLatest)list.scrollToItem(0)}
 val pendingText by remember{derivedStateOf{pending?.takeIf{ConversationPresentation.showPending(it.status)}?.let{runCatching{JSONObject(it.payload).optString("text")}.getOrDefault("")}.orEmpty()}}
 val sentAt by remember{derivedStateOf{pending?.let{runCatching{JSONObject(it.payload).optLong("created_at")*1000}.getOrDefault(0)}?:0}}
 val pendingQuestionVisible by remember{derivedStateOf{pending?.let{receipt->messages.any{message->message.role=="question"&&QuestionPresentation.parse(message)?.let{QuestionPresentation.matchesReply(receipt.payload,it.id)}==true}}==true}}
 val showPendingText by remember{derivedStateOf{!pendingQuestionVisible&&pending!=null&&ConversationPresentation.showPendingText(pending!!.status,pendingText,messages.any{it.role=="user"&&it.text==pendingText&&it.ordinal>=sentAt})}}
 val runs by remember{derivedStateOf{messageRuns(if(showPendingText)listOf(MessageRow(id,"pending:"+pending!!.requestId,"","user",pendingText,"",sentAt,pendingText.length,sentAt))+messages else messages)}}
 ConversationViewport(list=list,header=header,atLatest=atLatest,newest={newest()},refreshing=refreshing,refresh=refresh,loadingOlder=loadingHistory||(blocked&&runCatching{JSONObject(pending!!.payload).optString("kind")=="history"}.getOrDefault(false)),canLoadOlder=ready&&!currentBusy&&!blocked&&(nextPage!=null||currentThread?.historyCursor!=null||historyError),loadOlder={loadOlder()},composer={
  Column(Modifier.fillMaxWidth().composerMargins(composerMotion)) {
   Surface(color=MaterialTheme.colorScheme.surface,shape=RoundedCornerShape(26.dp),shadowElevation=2.dp,modifier=Modifier.fillMaxWidth().semantics{testTagsAsResourceId=true}.testTag("conversation_composer")) {
    Box(Modifier.composerBodyFrame(composerMotion)) {
     BasicTextField(value=draft,onValueChange={value->if(value.toByteArray().size<=32000){draft=value;repo.saveDraft(id,value)}},enabled=draftReady&&!currentBusy,visualTransformation=if(composerFocused)VisualTransformation.None else CompactDraftTransformation,maxLines=if(composerFocused)6 else 1,textStyle=TextStyle(color=MaterialTheme.colorScheme.onSurface,fontSize=16.sp,lineHeight=24.sp),cursorBrush=SolidColor(MaterialTheme.colorScheme.onSurface),modifier=Modifier.align(Alignment.TopStart).fillMaxWidth().padding(end=48.dp).heightIn(min=48.dp).onFocusChanged{composerFocused=it.isFocused},decorationBox={inner->Box(contentAlignment=Alignment.CenterStart){if(draft.isEmpty())Text("发送消息",fontSize=16.sp,color=MaterialTheme.colorScheme.onSurfaceVariant);inner()}})
     CompositionLocalProvider(LocalMinimumInteractiveComponentSize provides 0.dp) {
     BridgeIconButton(enabled=!currentBusy&&connected&&!blocked&&currentThread?.canSend==true&&currentThread?.online==true&&draft.isNotBlank(),onClick={focus.clearFocus();newest();work{val sent=draft;val result=repo.submit(currentThread!!,sent);if(composerSubmitted(result.status)&&draft==sent){draft="";repo.saveDraft(id,"")}}},modifier=Modifier.align(Alignment.BottomEnd),filled=true){Icon(BridgeIcons.Up,"发送",Modifier.size(20.dp))}
     }
    }
   }

  }
 },composerMotion=composerMotion,loading=messages.isEmpty()&&!ready) {
    if(!pendingQuestionVisible&&pending!=null&&ConversationPresentation.showPending(pending!!.status))item(key="pending:${pending!!.requestId}"){
     val text=runCatching{JSONObject(pending!!.payload).optString("text")}.getOrDefault("")
     PendingMessage(pending!!,captureMode,false,text,work,repo,taskIsRunning(currentThread)&&!currentBusy&&connected)
    }
    items(runs,key={it.key},contentType={it.role}){run->MessageRunCard(repo,run,list,work)}
    if(messages.isEmpty()&&ready)item{Column(Modifier.fillMaxWidth().padding(vertical=100.dp),horizontalAlignment=Alignment.CenterHorizontally){Text(if(currentThread?.localOnly==true)"开始新对话"else"从这里继续",fontSize=22.sp,fontWeight=FontWeight.SemiBold);Text(if(currentThread?.localOnly==true)"${threadProjectLabel(currentThread!!)} · 发送第一条消息开始"else"电脑保存的消息会显示在此对话中",fontSize=13.sp,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.padding(top=12.dp))}}
    if(historyError)item(key="history_error"){Text("历史加载失败，拉动顶部可重试",fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.fillMaxWidth())}

 }
}

@Composable private fun PendingMessage(pending:Pending,captureMode:Boolean,showText:Boolean,text:String,work:((suspend()->Unit))->Unit,repo:Repository,canSteer:Boolean=false) {
 Column(Modifier.fillMaxWidth(),horizontalAlignment=Alignment.End) {
  if(showText){UserBubble(QuestionPresentation.visibleReply(text));Text(messageTimestamp(runCatching{JSONObject(pending.payload).optLong("created_at")*1000}.getOrDefault(0)),fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.padding(top=4.dp))}
  Text(ReceiptPresentation.label(pending.status,captureMode),fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.padding(top=8.dp))
  pending.error?.let{Text(ReceiptPresentation.errorLabel(it),fontSize=12.sp,lineHeight=18.sp,color=MaterialTheme.colorScheme.error,modifier=Modifier.padding(top=4.dp))}
  if(canSteerPending(pending.status,pending.error,canSteer))TextButton(onClick={work{repo.steerPending(pending)}}){Text("作为引导发送",fontSize=12.sp)}
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
  if(message.role=="question")QuestionCard(repo,message,work)else if(message.role=="user")UserMessageWithImages(repo,message,QuestionPresentation.visibleReply(text)){UserBubble(it)}else MessageBody(text)
  if(message.characters>16000) {
   Row(Modifier.fillMaxWidth().padding(top=12.dp),verticalAlignment=Alignment.CenterVertically){TextButton(enabled=offset>0,onClick={work{val next=maxOf(0,offset-16000);text=repo.chunk(message,next);offset=next}}){Text("上一段",fontSize=12.sp)};Text("${offset+1}–${minOf(offset+16000,message.characters)} / ${message.characters}",fontSize=11.sp,color=MaterialTheme.colorScheme.onSurfaceVariant);TextButton(enabled=offset+16000<message.characters,onClick={work{val next=offset+16000;text=repo.chunk(message,next);offset=next}}){Text("下一段",fontSize=12.sp)}}
  }
 }
}

@Composable private fun NewConversationDialog(hosts:List<HostRow>,threads:List<ThreadRow>,catalog:List<ProjectRow>,remote:List<NewConversationProject>,busy:Boolean,dismiss:()->Unit,create:(HostRow,NewConversationProject)->Unit){
 var hostId by androidx.compose.runtime.saveable.rememberSaveable{mutableStateOf(hosts.firstOrNull()?.id.orEmpty())}
 var projectKey by androidx.compose.runtime.saveable.rememberSaveable{mutableStateOf("")}
 var choosing by androidx.compose.runtime.saveable.rememberSaveable{mutableStateOf("")}
 val host=hosts.find{it.id==hostId}
 val paths=remember(hostId,threads,catalog,remote){newConversationProjects(hostId,catalog,remote,threads)}
 val project=paths.find{it.key==projectKey}?:paths.find{it.path.isEmpty()&&it.hostId==hostId}
 val executor=hosts.find{it.id==project?.hostId}
 fun label(destination:NewConversationProject?)=destination?.let{it.name.takeIf{n->n.isNotBlank()}?:projectLabel(it.path)}?:"其他"
 fun detail(destination:NewConversationProject)=if(destination.path.isEmpty())"未归属项目"else if(destination.hostId!=hostId)"${hosts.find{it.id==destination.hostId}?.let(::hostLabel)?:"远端设备"} · ${destination.path}"else destination.path
 AlertDialog(shape=BridgeShapes.Panel,onDismissRequest={if(!busy)dismiss()},title={Row(verticalAlignment=Alignment.CenterVertically){if(choosing.isNotEmpty())BridgeIconButton(onClick={choosing=""}){Icon(BridgeIcons.Back,"返回新建对话")};Text(when(choosing){"device"->"选择设备";"project"->"选择项目";else->"新建对话"},fontSize=20.sp)}},text={
  AnimatedContent(targetState=choosing,transitionSpec={
   (fadeIn(tween(BridgeMotion.Fade))+slideInHorizontally(tween(BridgeMotion.Icon)){if(targetState.isEmpty())-16 else 16}) togetherWith
    (fadeOut(tween(BridgeMotion.Press))+slideOutHorizontally(tween(BridgeMotion.Icon)){if(targetState.isEmpty())16 else -16}) using SizeTransform(clip=true,sizeAnimationSpec={_,_->tween(BridgeMotion.Icon,easing=FastOutSlowInEasing)})
  },label="new conversation choice"){page->
   if(page.isNotEmpty())LazyColumn(Modifier.fillMaxWidth().heightIn(max=360.dp)){
    if(page=="device")items(hosts,key={it.id}){device->SelectionOption(hostLabel(device),if(device.online)"已连接"else"离线",device.id==hostId,BridgeIcons.Computer){hostId=device.id;projectKey="";choosing=""}}
    else items(paths,key={it.key}){path->SelectionOption(label(path),detail(path),path==project,BridgeIcons.Folder){projectKey=path.key;choosing=""}}
   }else Column(verticalArrangement=Arrangement.spacedBy(12.dp)){
    SelectionField("设备",host?.let(::hostLabel)?:"暂无设备",BridgeIcons.Computer,!busy&&hosts.isNotEmpty()){choosing="device"}
    SelectionField("项目",label(project),BridgeIcons.Folder,!busy&&host!=null){choosing="project"}
    Text(if(project!=null&&project.hostId!=hostId)"将在 ${executor?.let(::hostLabel)?:"远端设备"} 上创建对话。"else"选择设备与项目，开始新的对话。",fontSize=13.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)
   }
  }
 },confirmButton={if(choosing.isEmpty())TextButton(enabled=executor!=null&&project!=null&&!busy,onClick={if(executor!=null&&project!=null)create(executor,project)}){Text("建立")}},dismissButton={if(choosing.isEmpty())TextButton(enabled=!busy,onClick=dismiss){Text("取消")}})
}
@Composable private fun SelectionField(label:String,value:String,icon:androidx.compose.ui.graphics.vector.ImageVector,enabled:Boolean,select:()->Unit){
 Surface(color=MaterialTheme.colorScheme.surfaceContainerHigh,shape=RoundedCornerShape(16.dp),modifier=Modifier.fillMaxWidth().softClickable(enabled=enabled,onClick=select)){
  Row(Modifier.heightIn(min=64.dp).padding(horizontal=16.dp,vertical=12.dp),verticalAlignment=Alignment.CenterVertically){Icon(icon,null,Modifier.size(20.dp),tint=MaterialTheme.colorScheme.onSurfaceVariant);Spacer(Modifier.width(12.dp));Column(Modifier.weight(1f)){Text(label,fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant);Text(value,fontSize=15.sp,maxLines=2,overflow=TextOverflow.Ellipsis)};Icon(BridgeIcons.Chevron,null,Modifier.size(20.dp))}
 }
}
@Composable private fun SelectionOption(title:String,detail:String,selected:Boolean,icon:androidx.compose.ui.graphics.vector.ImageVector,select:()->Unit){
 val background by animateColorAsState(if(selected)MaterialTheme.colorScheme.surfaceContainerHigh else MaterialTheme.colorScheme.surfaceContainerHigh.copy(alpha=0f),tween(BridgeMotion.Release),label="selection")
 Row(Modifier.fillMaxWidth().background(background,BridgeShapes.Row).softClickable(onClick=select).semantics{this.selected=selected}.heightIn(min=64.dp).padding(horizontal=8.dp,vertical=12.dp),verticalAlignment=Alignment.CenterVertically){Icon(icon,null,Modifier.size(20.dp));Spacer(Modifier.width(12.dp));Column(Modifier.weight(1f)){Text(title,fontSize=15.sp,maxLines=2,overflow=TextOverflow.Ellipsis);Text(detail,fontSize=12.sp,maxLines=2,overflow=TextOverflow.Ellipsis,color=MaterialTheme.colorScheme.onSurfaceVariant)};if(selected)Icon(BridgeIcons.Check,"已选择",Modifier.size(20.dp))}
}
@Composable private fun MessageRunCard(repo:Repository,run:MessageRun,list:androidx.compose.foundation.lazy.LazyListState,work:((suspend()->Unit))->Unit){
 Column(Modifier.fillMaxWidth(),horizontalAlignment=if(run.role=="user")Alignment.End else Alignment.Start,verticalArrangement=Arrangement.spacedBy(8.dp)){
  var process=mutableListOf<MessageRow>()
  for(message in run.messages){
   if(isProcess(message))process.add(message)
   else {if(process.isNotEmpty()){ProcessMessages(repo,process.toList(),list,work);process=mutableListOf()};MessageCard(repo,message,work)}
  }
  if(process.isNotEmpty())ProcessMessages(repo,process.toList(),list,work)
  Text(run.timestamp,fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.padding(top=2.dp))
 }
}
@Composable private fun ProcessMessages(repo:Repository,messages:List<MessageRow>,list:androidx.compose.foundation.lazy.LazyListState,work:((suspend()->Unit))->Unit){
 var expanded by androidx.compose.runtime.saveable.rememberSaveable(messages.first().id){mutableStateOf(false)}
 val transition=updateTransition(expanded,label="process disclosure")
 val rail=MaterialTheme.colorScheme.outlineVariant
 var measured by remember{mutableStateOf<Float?>(null)}
 var anchor by remember{mutableStateOf<Float?>(null)}
 val angle by transition.animateFloat(transitionSpec={tween(BridgeMotion.Icon,easing=FastOutSlowInEasing)},label="process chevron"){if(it)180f else 0f}
 val surface by animateColorAsState(if(expanded)MaterialTheme.colorScheme.surfaceContainerLow else MaterialTheme.colorScheme.surfaceContainerLow.copy(alpha=0f),tween(BridgeMotion.Release),label="process capsule")
 LaunchedEffect(expanded){
  snapshotFlow{transition.currentState==expanded&&!transition.isRunning}.first{it}
  withFrameNanos{};anchor=null
 }
 // Preserve the control before drawing each resize frame. A post-layout
 // coroutine would briefly render the reverse list at its old bottom anchor.
 Column(Modifier.fillMaxWidth().onGloballyPositioned{coordinates->
  val current=coordinates.positionInWindow().y;measured=current
  anchor?.let{target->
   if(list.isScrollInProgress)anchor=null
   else {val shift=(target-current).roundToInt();if(abs(shift)>0)list.requestScrollToItem(list.firstVisibleItemIndex,list.firstVisibleItemScrollOffset+shift)}
  }
 }){
  Row(Modifier.heightIn(min=48.dp).background(surface,androidx.compose.foundation.shape.CircleShape).softClickable(shape=androidx.compose.foundation.shape.CircleShape){anchor=measured;expanded=!expanded}.semantics{contentDescription=if(expanded)"收起过程"else"展开过程";stateDescription=if(expanded)"已展开"else"已收起"}.padding(horizontal=12.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(8.dp)){
   Text("过程",fontSize=13.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)
   Text("${messages.size} 条",fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant)
   Icon(BridgeIcons.Chevron,null,Modifier.size(16.dp).graphicsLayer{rotationZ=90f+angle},tint=MaterialTheme.colorScheme.onSurfaceVariant)
  }
  transition.AnimatedVisibility(visible={it},enter=expandVertically(expandFrom=Alignment.Top,animationSpec=tween(BridgeMotion.Reveal,easing=FastOutSlowInEasing))+fadeIn(tween(BridgeMotion.Fade)),exit=shrinkVertically(shrinkTowards=Alignment.Top,animationSpec=tween(BridgeMotion.Icon,easing=FastOutSlowInEasing))+fadeOut(tween(BridgeMotion.Press))){
   Column(Modifier.padding(top=8.dp,start=12.dp).drawWithCache{onDrawBehind{drawLine(rail,Offset(-8.dp.toPx(),0f),Offset(-8.dp.toPx(),size.height),1.dp.toPx(),cap=androidx.compose.ui.graphics.StrokeCap.Round)}},verticalArrangement=Arrangement.spacedBy(12.dp)){messages.forEach{MessageCard(repo,it,work)}}
  }
 }
}

@Composable private fun ConversationSearchField(query:String,change:(String)->Unit,loading:Boolean,offline:Boolean){
 Column(Modifier.fillMaxWidth().padding(horizontal=12.dp,vertical=4.dp)){
  OutlinedTextField(value=query,onValueChange={change(it.take(64))},singleLine=true,placeholder={Text("搜索对话标题",fontSize=14.sp)},leadingIcon={Icon(BridgeIcons.Search,null)},trailingIcon={if(loading)CircularProgressIndicator(Modifier.size(18.dp),strokeWidth=2.dp)else if(query.isNotEmpty())BridgeIconButton(onClick={change("")}){Icon(BridgeIcons.Close,"清除搜索")}},modifier=Modifier.fillMaxWidth(),shape=RoundedCornerShape(16.dp),textStyle=TextStyle(fontSize=14.sp))
  if(offline&&query.isNotBlank())Text("离线 · 搜索本机已缓存标题",fontSize=12.sp,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.padding(top=4.dp))
 }
}

@Composable private fun BridgeMenuItem(title:String,icon:androidx.compose.ui.graphics.vector.ImageVector,action:()->Unit){
 val source=remember{androidx.compose.foundation.interaction.MutableInteractionSource()}
 Row(Modifier.fillMaxWidth().padding(horizontal=6.dp).softClickable(source=source,onClick=action).heightIn(min=48.dp).padding(horizontal=12.dp,vertical=8.dp),verticalAlignment=Alignment.CenterVertically){BridgePressIcon(icon,null,source,Modifier.size(20.dp),MaterialTheme.colorScheme.onSurfaceVariant);Spacer(Modifier.width(12.dp));Text(title,fontSize=14.sp)}
}
@Composable private fun SheetAction(title:String,icon:androidx.compose.ui.graphics.vector.ImageVector,danger:Boolean=false,action:()->Unit){
 val source=remember{androidx.compose.foundation.interaction.MutableInteractionSource()};val tint=if(danger)MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurface
 Row(Modifier.fillMaxWidth().softClickable(source=source,onClick=action).heightIn(min=48.dp).padding(horizontal=24.dp,vertical=12.dp),verticalAlignment=Alignment.CenterVertically){BridgePressIcon(icon,null,source,Modifier.size(22.dp),tint);Spacer(Modifier.width(16.dp));Text(title,color=tint)}
}
