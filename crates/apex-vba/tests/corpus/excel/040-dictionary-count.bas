' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 3
' corpus-prints: 2
' corpus-prints: 1
Sub Main()
    Dim d As Object, c As Range, k As Variant
    Set d = CreateObject("Scripting.Dictionary")
    Range("A1").Value = "red": Range("A2").Value = "blue": Range("A3").Value = "red"
    Range("A4").Value = "green": Range("A5").Value = "red"
    For Each c In Range("A1:A5")
        d(c.Value) = d(c.Value) + 1
    Next c
    Debug.Print d("red")
    Debug.Print d.Count - 1
    Debug.Print d("blue")
End Sub
