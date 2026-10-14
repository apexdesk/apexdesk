' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 3
Private counter As Long
Sub Bump()
    counter = counter + 1
End Sub
Sub Main()
    Bump
    Bump
    Bump
    Debug.Print counter
End Sub
