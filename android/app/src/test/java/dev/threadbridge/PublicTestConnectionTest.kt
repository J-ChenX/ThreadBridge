package dev.threadbridge

import org.junit.Assert.*
import org.junit.Test

class PublicTestConnectionTest {
 private val old="http://192.168.31.120:8787"
 private val public="https://test.example.com"
 @Test fun onlyThePreviouslyPairedConnectionMoves(){
  assertTrue(PublicTestConnection.shouldMigrate(old,"token",old,public,"host"))
  assertFalse(PublicTestConnection.shouldMigrate("https://another.example.com","token",old,public,"host"))
  assertFalse(PublicTestConnection.shouldMigrate(old,"",old,public,"host"))
  assertFalse(PublicTestConnection.shouldMigrate(old,"token",old,"http://test.example.com","host"))
  assertFalse(PublicTestConnection.shouldMigrate(old,"token",old,public,""))
 }
 @Test fun onlyThePublicTestTransportUsesPolling(){
  assertTrue(PublicTestConnection.usePolling(public,public))
  assertFalse(PublicTestConnection.usePolling(old,public))
  assertFalse(PublicTestConnection.usePolling("",""))
 }
 @Test fun anAuthenticatedDifferentHubCannotReplaceThePairedHub(){
  assertFalse(PublicTestConnection.matchesHost(listOf("another-host"),"host"))
  assertFalse(PublicTestConnection.matchesHost(listOf(""),""))
  assertTrue(PublicTestConnection.matchesHost(listOf("host"),"host"))
 }
 @Test fun knownExpiredEntrancesMoveWithoutRedirectingOtherPairings(){
  val expired="https://expired.example.com"
  val previous="$old,$expired"
  assertTrue(PublicTestConnection.shouldMigrate(expired,"token",previous,public,"host"))
  assertTrue(PublicTestConnection.shouldMigrate(old,"token",previous,public,"host"))
  assertFalse(PublicTestConnection.shouldMigrate("https://expired.example.com.evil","token",previous,public,"host"))
  assertFalse(PublicTestConnection.shouldMigrate(public,"token","$previous,$public",public,"host"))
 }
}
