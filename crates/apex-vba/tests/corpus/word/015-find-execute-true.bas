' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: True
' corpus-prints: False
Sub Main()
    Dim r As Range
    Set r = ActiveDocument.Content
    Debug.Print r.Find.Execute(FindText:="lazy")
    Set r = ActiveDocument.Content
    Debug.Print r.Find.Execute(FindText:="elephant")
End Sub
