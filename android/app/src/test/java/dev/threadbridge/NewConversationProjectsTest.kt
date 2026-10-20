package dev.threadbridge
import org.junit.Assert.*
import org.junit.Test
class NewConversationProjectsTest {
 @Test fun desktopRemoteProjectsRemainSelectableWithoutCachedThreads(){
  val remote=listOf(NewConversationProject("windows","lerrem","/remote/new","远端项目"))
  val result=newConversationProjects("windows",listOf(ProjectRow("windows","C:\\code\\local","本机项目")),remote,emptyList())
  assertTrue(result.contains(remote.single()))
  assertEquals("lerrem",result.first{it.name=="远端项目"}.hostId)
  assertEquals(3,result.size)
  assertFalse(newConversationProjects("nix",emptyList(),remote,emptyList()).any{it.path=="/remote/new"})
 }
 @Test fun pathsOnDifferentExecutorsAreDistinctAndRemoteNameWins(){
  val remote=listOf(NewConversationProject("windows","lerrem","/repo","远端名称"),NewConversationProject("windows","echova","/repo","另一个设备"))
  val cached=ThreadRow("id","title","lerrem","idle","",0,true,true,null,project="/repo",projectName="旧名称",projectKnown=true,displayHostId="windows")
  val result=newConversationProjects("windows",emptyList(),remote,listOf(cached))
  assertEquals(2,result.count{it.path=="/repo"})
  assertEquals("远端名称",result.first{it.hostId=="lerrem"}.name)
  assertEquals("windows",result.single{it.path.isEmpty()}.hostId)
 }
}
