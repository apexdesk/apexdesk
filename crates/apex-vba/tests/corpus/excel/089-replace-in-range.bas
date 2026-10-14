' corpus-source: generated
' corpus-flagged: no
' corpus-prints: new-1
' corpus-prints: new-2
Sub Main()
    Range("A1").Value = "old-1"
    Range("A2").Value = "old-2"
    Range("A1:A2").Replace What:="old", Replacement:="new", LookAt:=xlPart
    Debug.Print Range("A1").Value
    Debug.Print Range("A2").Value
End Sub
