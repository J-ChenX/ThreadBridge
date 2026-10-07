package dev.threadbridge

import org.junit.Assert.*
import org.junit.Test

class ConnectionConfigurationTest {
 private fun rejected(block:()->Unit){try{block();fail("must reject") }catch(_:IllegalArgumentException){}}
 @Test fun publicEndpointsRequireHttpsAndNoEmbeddedCredentials(){
  assertEquals("https://bridge.example.com",ConnectionConfiguration.server(" https://bridge.example.com/ ",false))
  for(url in listOf("http://bridge.example.com","https://secret@bridge.example.com","https://bridge.example.com?token=secret","https://bridge.example.com#secret","https://bridge.example.com:70000","garbage"))rejected{ConnectionConfiguration.server(url,true)}
 }
 @Test fun plainHttpRequiresExplicitLanAndAnActualPrivateIp(){
  assertEquals("http://10.0.2.2:8798",ConnectionConfiguration.server("http://10.0.2.2:8798",true))
  rejected{ConnectionConfiguration.server("http://10.0.2.2:8798",false)}
  for(url in listOf("http://10.999.2.2","http://172.32.1.1","http://192.169.1.1","http://192.168.1.2.evil.com"))rejected{ConnectionConfiguration.server(url,true)}
 }
 @Test fun blankKeyRetainsExistingCredentialButNeverInventsOne(){
  assertEquals("existing",ConnectionConfiguration.key(" ","existing"))
  assertEquals("replacement",ConnectionConfiguration.key("Bearer replacement","existing"))
  rejected{ConnectionConfiguration.key("","")}
  rejected{ConnectionConfiguration.key("secret\r\nInjected: yes","")}
  rejected{ConnectionConfiguration.key("x".repeat(4097),"")}
 }
 @Test fun anotherCollectionCannotOverwriteCachedConversations(){
  assertTrue(ConnectionConfiguration.sameCollection("","first-hub"))
  assertTrue(ConnectionConfiguration.sameCollection("hub","hub"))
  assertFalse(ConnectionConfiguration.sameCollection("hub","other-hub"))
  assertFalse(ConnectionConfiguration.sameCollection("hub",""))
 }
}
