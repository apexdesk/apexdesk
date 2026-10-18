' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: The
Sub Main()
    Selection.HomeKey wdStory
    Selection.MoveRight wdCharacter, 3, wdExtend
    Debug.Print Selection.Text
End Sub
