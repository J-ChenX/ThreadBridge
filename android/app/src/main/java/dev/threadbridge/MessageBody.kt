package dev.threadbridge

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.*
import androidx.compose.ui.text.font.*
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.commonmark.node.*
import org.commonmark.node.Text as TextNode
import org.commonmark.node.Paragraph as MarkdownParagraph
import org.commonmark.parser.Parser
import org.commonmark.ext.gfm.tables.*

internal object MessageFormatting {
 private val parser=Parser.builder().extensions(listOf(TablesExtension.create())).build()
 fun parse(text:String):Node=parser.parse(text)
 fun plain(node:Node):String=buildString {
  val queue=java.util.ArrayDeque<Node>();queue.add(node)
  while(queue.isNotEmpty()) { val n=queue.removeLast();when(n) {
   is TextNode->append(n.literal);is Code->append(n.literal);is HtmlInline->append(n.literal)
   is FencedCodeBlock->append(n.literal);is IndentedCodeBlock->append(n.literal);is HtmlBlock->append(n.literal)
   is SoftLineBreak,is HardLineBreak->append("\n")
   else->{var child=n.lastChild;while(child!=null){queue.add(child);child=child.previous}}
  } }
 }
 fun webLink(destination:String):Boolean=runCatching {
  val uri=java.net.URI(destination)
  uri.scheme?.lowercase() in setOf("https","http") && !uri.host.isNullOrEmpty() && uri.userInfo==null
 }.getOrDefault(false)
 fun inline(node:Node,code:SpanStyle,link:SpanStyle):AnnotatedString=buildAnnotatedString {
  fun visit(n:Node,depth:Int) {
   if(depth>32){append(plain(n));return}
   fun children(){var child=n.firstChild;while(child!=null){visit(child,depth+1);child=child.next}}
   when(n) {
    is TextNode->append(n.literal)
    is SoftLineBreak->append(" ")
    is HardLineBreak->append("\n")
    is Code->withStyle(code){append(n.literal)}
    is StrongEmphasis->withStyle(SpanStyle(fontWeight=FontWeight.SemiBold)){children()}
    is Emphasis->withStyle(SpanStyle(fontStyle=FontStyle.Italic)){children()}
    is Link->if(webLink(n.destination))withLink(LinkAnnotation.Url(n.destination,TextLinkStyles(style=link))){children()}else{children();append(" (${n.destination})")}
    is Image->{children();append(" (${n.destination})")}
    is HtmlInline->append(n.literal)
    else->children()
   }
  }
  visit(node,0)
 }
}
private fun Node.children():List<Node> = buildList {
 var node=this@children.firstChild
 while(node!=null){add(node);node=node.next}
}
@Composable internal fun MessageBody(text:String) {
 val document=remember(text){MessageFormatting.parse(text)}
 SelectionContainer{Column(Modifier.fillMaxWidth(),verticalArrangement=Arrangement.spacedBy(14.dp)){document.children().forEach{MarkdownBlock(it)}}}
}
@Composable private fun InlineText(node:Node,modifier:Modifier=Modifier,weight:FontWeight?=null,size:Int=16) {
 val colors=MaterialTheme.colorScheme
 val code=SpanStyle(fontFamily=FontFamily.Monospace,background=colors.surfaceContainerHigh)
 val link=SpanStyle(color=colors.onSurface,textDecoration=TextDecoration.Underline)
 val content=remember(node,code,link){MessageFormatting.inline(node,code,link)}
 Text(content,modifier=modifier,color=colors.onSurface,fontSize=size.sp,lineHeight=(size+10).sp,fontWeight=weight)
}
@Composable private fun MarkdownBlock(node:Node,depth:Int=0) {
 if(depth>32){Text(MessageFormatting.plain(node),fontSize=16.sp,lineHeight=26.sp);return}
 when(node) {
  is MarkdownParagraph->InlineText(node)
  is Heading->InlineText(node,Modifier.padding(top=6.dp),FontWeight.SemiBold,when(node.level){1->23;2->20;else->18})
  is BulletList->Column(verticalArrangement=Arrangement.spacedBy(10.dp)){node.children().forEach{item->Row{Text("•",Modifier.width(22.dp),fontSize=16.sp,lineHeight=26.sp);Column(Modifier.weight(1f),verticalArrangement=Arrangement.spacedBy(8.dp)){item.children().forEach{MarkdownBlock(it,depth+1)}}}}}
  is OrderedList->Column(verticalArrangement=Arrangement.spacedBy(10.dp)){node.children().forEachIndexed{index,item->Row{Text("${node.markerStartNumber+index}.",Modifier.width(28.dp),fontSize=16.sp,lineHeight=26.sp);Column(Modifier.weight(1f),verticalArrangement=Arrangement.spacedBy(8.dp)){item.children().forEach{MarkdownBlock(it,depth+1)}}}}}
  is FencedCodeBlock->CodeBlock(node.literal,node.info)
  is IndentedCodeBlock->CodeBlock(node.literal,"")
  is BlockQuote->Row(Modifier.fillMaxWidth()){Box(Modifier.width(3.dp).heightIn(min=26.dp).background(MaterialTheme.colorScheme.outlineVariant));Column(Modifier.padding(start=14.dp),verticalArrangement=Arrangement.spacedBy(10.dp)){node.children().forEach{MarkdownBlock(it,depth+1)}}}
  is ThematicBreak->HorizontalDivider(color=MaterialTheme.colorScheme.outlineVariant)
  is HtmlBlock->Text(node.literal,fontSize=16.sp,lineHeight=26.sp)
  is TableBlock->Surface(shape=RoundedCornerShape(12.dp),color=MaterialTheme.colorScheme.surfaceContainerLow){Column(Modifier.horizontalScroll(rememberScrollState())){node.children().forEach{section->section.children().forEach{row->Row{row.children().forEach{cell->InlineText(cell,Modifier.width(160.dp).padding(12.dp),if(section is TableHead)FontWeight.SemiBold else null,14)}};HorizontalDivider(color=MaterialTheme.colorScheme.outlineVariant)}}}}
  else->node.children().forEach{MarkdownBlock(it,depth+1)}
 }
}
@Composable private fun CodeBlock(text:String,language:String) {
 Surface(shape=RoundedCornerShape(14.dp),color=MaterialTheme.colorScheme.surfaceContainerLow,modifier=Modifier.fillMaxWidth()) {
  Column(Modifier.padding(14.dp)){
   if(language.isNotBlank())Text(language.substringBefore(' '),fontSize=11.sp,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.padding(bottom=10.dp))
   Text(text.trimEnd('\n'),fontFamily=FontFamily.Monospace,fontSize=13.sp,lineHeight=21.sp,modifier=Modifier.horizontalScroll(rememberScrollState()))
  }
 }
}
