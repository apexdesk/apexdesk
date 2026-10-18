' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: -1
Sub Main()
    Selection.HomeKey wdStory
    Selection.MoveRight Unit:=wdCharacter, Count:=3, Extend:=wdExtend
    Selection.Font.Bold = True
    Debug.Print ActiveDocument.Range(0, 3).Font.Bold
End Sub
