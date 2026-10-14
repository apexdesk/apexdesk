' corpus-kind: word
' corpus-source: generated
' corpus-expect: ok
' corpus-note: Styles.Add is not supported
' corpus-flagged: no
' corpus-prints: Callout
' corpus-prints: Callout
Sub Main()
    Dim st As Style
    Set st = ActiveDocument.Styles.Add(Name:="Callout", Type:=wdStyleTypeParagraph)
    st.Font.Italic = True
    ActiveDocument.Paragraphs(3).Style = "Callout"
    Debug.Print st.NameLocal
    Debug.Print ActiveDocument.Paragraphs(3).Style
End Sub
